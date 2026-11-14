//! Saved presets: named pipelines, stored as one JSON list by the caller
//! (the core keeps them in the database's key-value table).

use serde::{Deserialize, Serialize};

use crate::pipeline::Pipeline;

/// A named pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    /// Its name, shown in the preset menu.
    pub name: String,
    /// The steps.
    pub pipeline: Pipeline,
}

/// The saved presets, in the order the user saved them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Presets {
    /// Format version.
    #[serde(default = "one")]
    pub version: u32,
    /// The presets.
    #[serde(default)]
    pub presets: Vec<Preset>,
}

fn one() -> u32 {
    1
}

impl Presets {
    /// Reads the stored JSON. Unreadable text gives no presets (the caller
    /// keeps the text aside so nothing is lost).
    pub fn from_json(s: &str) -> Result<Presets, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// The JSON to store.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Adds a preset, or replaces the one with the same name (names compare
    /// ignoring case and surrounding spaces).
    pub fn save(&mut self, name: &str, pipeline: Pipeline) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a preset needs a name".into());
        }
        let p = Preset {
            name: name.to_string(),
            pipeline,
        };
        match self.position(name) {
            Some(i) => self.presets[i] = p,
            None => self.presets.push(p),
        }
        Ok(())
    }

    /// Removes a preset by name. Returns whether there was one.
    pub fn delete(&mut self, name: &str) -> bool {
        match self.position(name) {
            Some(i) => {
                self.presets.remove(i);
                true
            }
            None => false,
        }
    }

    /// The preset with this name.
    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.position(name).map(|i| &self.presets[i])
    }

    fn position(&self, name: &str) -> Option<usize> {
        let k = name.trim().to_lowercase();
        self.presets
            .iter()
            .position(|p| p.name.trim().to_lowercase() == k)
    }

    /// The presets every user starts with.
    pub fn defaults() -> Presets {
        use crate::pipeline::{CaseKind, ExtChange, Step, Target};
        Presets {
            version: 1,
            presets: vec![
                Preset {
                    name: "Photos by date taken".into(),
                    pipeline: Pipeline::of(vec![
                        Step::Template {
                            template: "{exif.date:%Y-%m-%d} {n:3}".into(),
                        },
                        Step::Extension {
                            change: ExtChange::Lower,
                        },
                    ]),
                },
                Preset {
                    name: "Numbered".into(),
                    pipeline: Pipeline::of(vec![Step::Template {
                        template: "{parent} {n:3}".into(),
                    }]),
                },
                Preset {
                    name: "Tidy names".into(),
                    pipeline: Pipeline::of(vec![
                        Step::Replace {
                            find: "[_]+".into(),
                            replace: " ".into(),
                            regex: true,
                            ignore_case: false,
                            first_only: false,
                            target: Target::Name,
                        },
                        Step::Trim {
                            mode: crate::pipeline::TrimMode::Whitespace,
                            target: Target::Name,
                        },
                        Step::Case {
                            case: CaseKind::Title,
                            target: Target::Name,
                        },
                    ]),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Compiled, Step};

    #[test]
    fn save_replace_delete() {
        let mut ps = Presets::default();
        let p = Pipeline::of(vec![Step::Template {
            template: "{n}".into(),
        }]);
        assert!(ps.save("  ", p.clone()).is_err());
        ps.save("Mine", p.clone()).unwrap();
        ps.save("mine ", Pipeline::default()).unwrap();
        assert_eq!(ps.presets.len(), 1);
        assert_eq!(ps.get("MINE").unwrap().pipeline, Pipeline::default());
        let back = Presets::from_json(&ps.to_json()).unwrap();
        assert_eq!(back, ps);
        assert!(ps.delete("Mine"));
        assert!(!ps.delete("Mine"));
    }

    #[test]
    fn defaults_compile() {
        for p in Presets::defaults().presets {
            Compiled::new(&p.pipeline).unwrap();
        }
        assert!(Presets::from_json("not json").is_err());
        assert_eq!(Presets::from_json("{}").unwrap().version, 1);
    }
}
