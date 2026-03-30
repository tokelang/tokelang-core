use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

const TIKTOKEN_WORKER_SCRIPT: &str = "import sys\n\
import tiktoken\n\
enc = tiktoken.get_encoding('cl100k_base')\n\
stdin = sys.stdin.buffer\n\
stdout = sys.stdout\n\
while True:\n\
    header = stdin.readline()\n\
    if not header:\n\
        break\n\
    try:\n\
        size = int(header)\n\
    except ValueError:\n\
        stdout.write('ERR\\n')\n\
        stdout.flush()\n\
        continue\n\
    payload = stdin.read(size)\n\
    if len(payload) != size:\n\
        break\n\
    text = payload.decode('utf-8')\n\
    stdout.write(f\"{len(enc.encode(text))}\\n\")\n\
    stdout.flush()\n";

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
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);
        let mut worker = Self {
            child,
            stdin,
            stdout,
        };
        worker.count("")?;
        Some(worker)
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
