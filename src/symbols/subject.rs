use std::collections::HashMap;

/// Greedy phrase match result from the subject table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectMatch {
    pub surface: String,
    pub canonical: String,
    pub consumed: usize,
}

/// Phrase-to-abbreviation dictionary for common semantic entities.
pub struct SubjectTable {
    forward: HashMap<String, String>,
    reverse: HashMap<String, String>,
}

impl SubjectTable {
    pub fn default_table() -> Self {
        let pairs = [
            ("quantum entanglement", "QENT"),
            ("quantum mechanics", "QMCH"),
            ("quantum computing", "QCMP"),
            ("machine learning", "ML"),
            ("deep learning", "DL"),
            ("neural network", "NN"),
            ("neural networks", "NN"),
            ("artificial intelligence", "AI"),
            ("natural language processing", "NLP"),
            ("computer vision", "CV"),
            ("reinforcement learning", "RL"),
            ("software architecture", "SARCH"),
            ("database", "DB"),
            ("api", "API"),
            ("microservices", "MSVC"),
            ("design patterns", "DPAT"),
            ("data structures", "DS"),
            ("algorithms", "ALGO"),
            ("operating system", "OS"),
            ("distributed systems", "DSYS"),
            ("article", "ARTICLE"),
            ("report", "REPORT"),
            ("essay", "ESSAY"),
            ("code", "CODE"),
            ("email", "EMAIL"),
            ("documentation", "DOCS"),
            ("data", "DATA"),
            ("dataset", "DSET"),
            ("statistics", "STAT"),
            ("metrics", "METR"),
            ("trend", "TREND"),
            ("trends", "TRENDS"),
            ("benchmark", "BENCH"),
            ("climate change", "CLMCH"),
            ("blockchain", "BLKCH"),
            ("cybersecurity", "CSEC"),
            ("cloud computing", "CLOUD"),
            ("internet of things", "IOT"),
            ("virtual reality", "VR"),
            ("augmented reality", "AR"),
        ];

        let mut forward = HashMap::with_capacity(pairs.len());
        let mut reverse = HashMap::with_capacity(pairs.len());

        for (phrase, canonical) in pairs {
            forward.insert(phrase.to_lowercase(), canonical.to_string());
            reverse.insert(canonical.to_string(), phrase.to_string());
        }

        Self { forward, reverse }
    }

    pub fn abbreviate(&self, phrase: &str) -> Option<&str> {
        self.forward.get(&phrase.to_lowercase()).map(String::as_str)
    }

    pub fn expand(&self, canonical: &str) -> Option<&str> {
        self.reverse.get(canonical).map(String::as_str)
    }

    pub fn register(&mut self, phrase: &str, canonical: &str) {
        self.forward
            .insert(phrase.to_lowercase(), canonical.to_string());
        self.reverse
            .insert(canonical.to_string(), phrase.to_lowercase());
    }

    pub fn longest_match_from(&self, words: &[String], start: usize) -> Option<SubjectMatch> {
        if start >= words.len() {
            return None;
        }

        let max_len = (words.len() - start).min(4);
        for len in (1..=max_len).rev() {
            let phrase = words[start..start + len].join(" ").to_lowercase();
            if let Some(canonical) = self.forward.get(&phrase) {
                return Some(SubjectMatch {
                    surface: words[start..start + len].join(" "),
                    canonical: canonical.clone(),
                    consumed: len,
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviate_and_expand() {
        let table = SubjectTable::default_table();
        assert_eq!(table.abbreviate("quantum entanglement"), Some("QENT"));
        assert_eq!(table.expand("QENT"), Some("quantum entanglement"));
    }

    #[test]
    fn longest_match_prefers_longer_phrase() {
        let table = SubjectTable::default_table();
        let words = vec![
            "natural".to_string(),
            "language".to_string(),
            "processing".to_string(),
            "pipeline".to_string(),
        ];
        let matched = table.longest_match_from(&words, 0).unwrap();
        assert_eq!(matched.canonical, "NLP");
        assert_eq!(matched.consumed, 3);
    }
}
