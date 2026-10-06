//! Call log of the link-seam stubs (port of `test/native/glue/stubs/stub_log`): every stub env
//! owns one; a stubbed call appends "name(arg, arg)" (integers in decimal, masks in hex) and the
//! tests compare whole logs, as the C++ `stub::calls`.

#[derive(Default, Debug)]
pub struct CallLog {
    pub calls: Vec<String>,
}

impl CallLog {
    pub fn log(&mut self, entry: impl Into<String>) {
        self.calls.push(entry.into());
    }

    /// The entries of one function ("name(" prefix).
    pub fn calls_of(&self, name: &str) -> Vec<String> {
        let prefix = format!("{name}(");
        self.calls
            .iter()
            .filter(|c| c.starts_with(&prefix))
            .cloned()
            .collect()
    }

    pub fn clear(&mut self) {
        self.calls.clear();
    }
}
