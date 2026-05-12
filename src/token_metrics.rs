use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

// Raw string literal — `\n\` line continuations in the previous form silently
// stripped indentation from the Python body and made the script unparseable,
// so every routing decision since v0.9.0 fell back to `proxy_count`.
const TIKTOKEN_WORKER_SCRIPT: &str = r#"import sys
import tiktoken
enc = tiktoken.get_encoding('cl100k_base')
stdin = sys.stdin.buffer
stdout = sys.stdout
while True:
    header = stdin.readline()
    if not header:
        break
    try:
        size = int(header)
    except ValueError:
        stdout.write('ERR\n')
        stdout.flush()
        continue
    payload = stdin.read(size)
    if len(payload) != size:
        break
    text = payload.decode('utf-8')
    stdout.write(f"{len(enc.encode(text))}\n")
    stdout.flush()
"#;

static TIKTOKEN_WORKER: OnceLock<Mutex<TiktokenWorkerState>> = OnceLock::new();

#[derive(Debug, Clone)]
pub enum Tokenizer {
    TiktokenCl100k,
    Proxy,
}

impl Tokenizer {
    pub fn detect() -> Self {
        if tiktoken_worker_available() {
            Self::TiktokenCl100k
        } else {
            Self::Proxy
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::TiktokenCl100k => "cl100k_base via persistent python tiktoken worker",
            Self::Proxy => "offline proxy tokenizer",
        }
    }

    pub fn count(&self, text: &str) -> usize {
        match self {
            Self::TiktokenCl100k => count_with_tiktoken(text).unwrap_or_else(|| proxy_count(text)),
            Self::Proxy => proxy_count(text),
        }
    }
}

#[derive(Debug)]
enum TiktokenWorkerState {
    Uninitialized,
    Available(TiktokenWorker),
    Unavailable,
}

#[derive(Debug)]
struct TiktokenWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl TiktokenWorker {
    fn spawn() -> Option<Self> {
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(TIKTOKEN_WORKER_SCRIPT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);
        let mut stderr = child.stderr.take();
        let mut worker = Self {
            child,
            stdin,
            stdout,
        };
        if worker.count("").is_some() {
            return Some(worker);
        }
        // Handshake failed. The child has likely exited (stdout closed mid-handshake);
        // drain its stderr so the cause isn't invisible — that silence is what hid
        // the v0.9.0–v0.9.2 indentation bug in production.
        let mut buf = String::new();
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_string(&mut buf);
        }
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            eprintln!(
                "[tokelang] tiktoken worker handshake failed; falling back to proxy tokenizer (no stderr captured)"
            );
        } else {
            eprintln!(
                "[tokelang] tiktoken worker handshake failed; falling back to proxy tokenizer. python stderr:\n{}",
                trimmed
            );
        }
        None
    }

    fn count(&mut self, text: &str) -> Option<usize> {
        writeln!(self.stdin, "{}", text.len()).ok()?;
        self.stdin.write_all(text.as_bytes()).ok()?;
        self.stdin.flush().ok()?;

        let mut line = String::new();
        self.stdout.read_line(&mut line).ok()?;
        line.trim().parse().ok()
    }
}

impl Drop for TiktokenWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn count_with_tiktoken(text: &str) -> Option<usize> {
    let state = tiktoken_worker();
    let mut guard = state.lock().ok()?;
    if !ensure_worker(&mut guard) {
        return None;
    }

    let count = match &mut *guard {
        TiktokenWorkerState::Available(worker) => worker.count(text),
        TiktokenWorkerState::Uninitialized | TiktokenWorkerState::Unavailable => None,
    };
    if count.is_some() {
        return count;
    }

    *guard = TiktokenWorkerState::Uninitialized;
    if !ensure_worker(&mut guard) {
        return None;
    }

    match &mut *guard {
        TiktokenWorkerState::Available(worker) => worker.count(text),
        TiktokenWorkerState::Uninitialized | TiktokenWorkerState::Unavailable => None,
    }
}

fn tiktoken_worker_available() -> bool {
    let state = tiktoken_worker();
    let mut guard = match state.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    ensure_worker(&mut guard)
}

fn tiktoken_worker() -> &'static Mutex<TiktokenWorkerState> {
    TIKTOKEN_WORKER.get_or_init(|| Mutex::new(TiktokenWorkerState::Uninitialized))
}

fn ensure_worker(state: &mut TiktokenWorkerState) -> bool {
    match state {
        TiktokenWorkerState::Available(_) => true,
        TiktokenWorkerState::Unavailable => false,
        TiktokenWorkerState::Uninitialized => {
            *state = match TiktokenWorker::spawn() {
                Some(worker) => TiktokenWorkerState::Available(worker),
                None => TiktokenWorkerState::Unavailable,
            };
            matches!(state, TiktokenWorkerState::Available(_))
        }
    }
}

fn proxy_count(text: &str) -> usize {
    let mut count = 0usize;
    let mut in_word = false;

    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' || character == '-' {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if !character.is_whitespace() {
                count += 1;
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::TIKTOKEN_WORKER_SCRIPT;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn worker_script_parses_as_valid_python() {
        // Guards against regressions where the Python script's indentation gets stripped
        // by Rust string-literal rules (the v0.9.0–v0.9.2 bug). Runs anywhere `python3`
        // is on PATH; does not require tiktoken to be installed.
        let mut child = match Command::new("python3")
            .args(["-c", "import sys, ast; ast.parse(sys.stdin.read())"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                eprintln!("SKIP: python3 not available");
                return;
            }
        };
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(TIKTOKEN_WORKER_SCRIPT.as_bytes())
            .unwrap();
        drop(child.stdin.take());
        let out = child.wait_with_output().expect("wait_with_output");
        assert!(
            out.status.success(),
            "TIKTOKEN_WORKER_SCRIPT is not valid Python:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
