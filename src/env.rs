// A snapshot of environment variables, passed explicitly so tests can inject
// exactly the variables a function may read (mirroring the JS modules, which
// took an `env` object defaulting to process.env). Spawned commands receive
// this snapshot as their entire environment.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Env {
    vars: BTreeMap<String, String>,
}

impl Env {
    pub fn real() -> Self {
        Env {
            vars: std::env::vars().collect(),
        }
    }

    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Env {
            vars: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(|s| s.as_str())
    }

    /// Like JS truthiness on an env string: set and non-empty.
    pub fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| !v.is_empty())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.vars.insert(key.to_string(), value.to_string());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.vars.iter()
    }
}
