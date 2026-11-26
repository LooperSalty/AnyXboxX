//! Toast notifications: short messages that fade in, stay a few seconds and fade out. The queue
//! and the timing are plain data; the window draws what `visible` returns.

/// Seconds a toast stays, by kind.
const LIFE_INFO: f64 = 4.5;
const LIFE_ERROR: f64 = 8.0;
const FADE_IN: f64 = 0.2;
const FADE_OUT: f64 = 0.5;
/// Toasts on screen at once; the oldest goes first.
const MAX_VISIBLE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub text: String,
    pub kind: Kind,
    pub born: f64,
}

impl Toast {
    fn life(&self) -> f64 {
        match self.kind {
            Kind::Error => LIFE_ERROR,
            _ => LIFE_INFO,
        }
    }

    /// Opacity at `now`: 0 before it exists and after it is gone.
    pub fn alpha(&self, now: f64) -> f32 {
        let age = now - self.born;
        if age < 0.0 || age >= self.life() {
            return 0.0;
        }
        let fade_in = (age / FADE_IN).min(1.0);
        let fade_out = ((self.life() - age) / FADE_OUT).min(1.0);
        fade_in.min(fade_out) as f32
    }
}

#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    /// Adds a toast; the same text already on screen only restarts its timer.
    pub fn push(&mut self, text: impl Into<String>, kind: Kind, now: f64) {
        let text = text.into();
        self.items.retain(|t| t.text != text);
        self.items.push(Toast {
            text,
            kind,
            born: now,
        });
        while self.items.len() > MAX_VISIBLE {
            self.items.remove(0);
        }
    }

    /// Drops the toasts whose time is up.
    pub fn prune(&mut self, now: f64) {
        self.items.retain(|t| now - t.born < t.life());
    }

    /// The toasts to draw, oldest first.
    pub fn visible(&self) -> &[Toast] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Picks the kind of a message the launcher's actions produce.
pub fn classify(text: &str) -> Kind {
    let lower = text.to_lowercase();
    let failed = [
        "could not",
        "cannot",
        "stopped with",
        "not found",
        "type the",
        "no cache folder",
        "refused",
        "error",
    ];
    if failed.iter().any(|w| lower.contains(w)) {
        Kind::Error
    } else if ["finished", "removed", "exited normally", "saved"]
        .iter()
        .any(|w| lower.contains(w))
    {
        Kind::Success
    } else {
        Kind::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reads_the_launchers_messages() {
        assert_eq!(classify("Shader preloading finished."), Kind::Success);
        assert_eq!(classify("VIctory exited normally."), Kind::Success);
        assert_eq!(
            classify("VIctory stopped with exit code 3: see the log."),
            Kind::Error
        );
        assert_eq!(classify("Could not run: boom"), Kind::Error);
        assert_eq!(classify("Type the game folder first."), Kind::Error);
        assert_eq!(classify("No shader cache to remove."), Kind::Info);
    }

    #[test]
    fn a_toast_fades_in_stays_and_fades_out() {
        let t = Toast {
            text: "x".into(),
            kind: Kind::Info,
            born: 10.0,
        };
        assert_eq!(t.alpha(9.0), 0.0);
        assert!(t.alpha(10.05) > 0.0 && t.alpha(10.05) < 1.0);
        assert_eq!(t.alpha(12.0), 1.0);
        assert!(t.alpha(14.3) < 1.0 && t.alpha(14.3) > 0.0);
        assert_eq!(t.alpha(15.0), 0.0);
    }

    #[test]
    fn errors_stay_longer() {
        let info = Toast {
            text: "a".into(),
            kind: Kind::Info,
            born: 0.0,
        };
        let error = Toast {
            text: "b".into(),
            kind: Kind::Error,
            born: 0.0,
        };
        assert_eq!(info.alpha(6.0), 0.0);
        assert_eq!(error.alpha(6.0), 1.0);
    }

    #[test]
    fn the_queue_dedupes_caps_and_prunes() {
        let mut toasts = Toasts::default();
        toasts.push("same", Kind::Info, 0.0);
        toasts.push("same", Kind::Info, 1.0);
        assert_eq!(toasts.visible().len(), 1);
        assert_eq!(toasts.visible()[0].born, 1.0);
        for i in 0..6 {
            toasts.push(format!("t{i}"), Kind::Info, 2.0);
        }
        assert_eq!(toasts.visible().len(), MAX_VISIBLE);
        assert_eq!(toasts.visible()[0].text, "t2");
        toasts.prune(100.0);
        assert!(toasts.is_empty());
    }
}
