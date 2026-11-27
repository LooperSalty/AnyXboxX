//! Optional NVIDIA Streamline features selected on the Play page.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub dlss: bool,
    pub framegen: bool,
    pub directory: String,
}

impl Options {
    pub fn append_env(&self, env: &mut Vec<(String, String)>) {
        if self.dlss {
            env.push(("VICTORY_DLSS".into(), "1".into()));
        }
        if self.framegen {
            env.push(("VICTORY_FRAMEGEN".into(), "1".into()));
            env.push(("VICTORY_THIN".into(), "1".into()));
        }
        let dir = self.directory.trim().trim_matches('"');
        if !dir.is_empty() {
            env.push(("VICTORY_STREAMLINE_DIR".into(), dir.into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::options::LaunchOptions;

    #[test]
    fn play_options_survive_restart_and_reach_the_environment() {
        let mut options: LaunchOptions = toml::from_str("").unwrap();
        assert!(!options.streamline.dlss && !options.streamline.framegen);
        options.streamline.dlss = true;
        options.streamline.directory = r"C:\NVIDIA SDK\bin\x64".into();
        let env = options.env_pairs();
        assert!(env.contains(&("VICTORY_DLSS".into(), "1".into())));
        assert!(
            !env.iter()
                .any(|(key, _)| key == "VICTORY_THIN" || key == "VICTORY_FRAMEGEN")
        );
        options.streamline.framegen = true;
        let saved = toml::to_string(&options).unwrap();
        let restored: LaunchOptions = toml::from_str(&saved).unwrap();
        let env = restored.env_pairs();
        assert_eq!(restored, options);
        assert!(env.contains(&("VICTORY_FRAMEGEN".into(), "1".into())));
        assert!(env.contains(&("VICTORY_THIN".into(), "1".into())));
        assert!(env.contains(&(
            "VICTORY_STREAMLINE_DIR".into(),
            options.streamline.directory
        )));
    }
}
