//! Kernel command-line switches for the Asterinas spike (plan step 34). Pure logic.
//!
//!   allternit.state=none        skip the state partition mount
//!   allternit.model=/some/path  point the model service's `-m` at this path
//!
//! With neither switch the config is untouched, so the Linux image behaves as before.

use crate::config::Config;

/// Applies the switches in `cmdline` to `cfg`. Returns true if anything changed.
pub fn apply(cfg: &mut Config, cmdline: &str) -> bool {
    let mut changed = false;
    for word in cmdline.split_whitespace() {
        if word == "allternit.state=none" {
            cfg.state_device = None;
            changed = true;
        } else if let Some(path) = word.strip_prefix("allternit.model=") {
            if path.is_empty() {
                continue;
            }
            for svc in cfg.services.iter_mut().filter(|s| s.name == "model") {
                if let Some(i) = svc.args.iter().position(|a| a == "-m") {
                    if i + 1 < svc.args.len() {
                        svc.args[i + 1] = path.to_string();
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;

    fn model_path(c: &Config) -> String {
        let m = c.services.iter().find(|s| s.name == "model").unwrap();
        let i = m.args.iter().position(|a| a == "-m").unwrap();
        m.args[i + 1].clone()
    }

    #[test]
    fn no_switch_changes_nothing() {
        let mut c = default_config();
        let before = c.clone();
        assert!(!apply(&mut c, "console=ttyS0 rdinit=/sbin/init panic=-1"));
        assert_eq!(c, before);
    }

    #[test]
    fn state_none_clears_device() {
        let mut c = default_config();
        assert!(c.state_device.is_some());
        assert!(apply(&mut c, "rdinit=/sbin/init allternit.state=none"));
        assert!(c.state_device.is_none());
    }

    #[test]
    fn model_switch_moves_the_path() {
        let mut c = default_config();
        assert_eq!(model_path(&c), "/state/model/model.gguf");
        assert!(apply(&mut c, "allternit.model=/model/model.gguf"));
        assert_eq!(model_path(&c), "/model/model.gguf");
    }
}
