//! Read admission prevents element numbers crossing browser changes or binding generations.
use super::{Op, preference::Backend};

/// The binding keeps only whether its last read still names the selected backend.
#[derive(Default)]
pub(super) struct References {
    generation: u64,
    backend: Option<Backend>,
    valid: bool,
}

/// A completed old read cannot resurrect references after the person changes browsers.
pub(super) struct ReadStamp {
    generation: u64,
    backend: Backend,
    read: bool,
}

impl References {
    /// Browser switches invalidate references even if the person switches away and back.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.valid = false;
    }

    /// Actions require a successful read in this binding's current backend.
    pub fn begin(&mut self, backend: Backend, op: &Op) -> Result<ReadStamp, String> {
        if self.backend.is_some_and(|previous| previous != backend) {
            self.invalidate();
        }
        self.backend = Some(backend);
        if matches!(op, Op::Click { .. } | Op::Type { .. })
            && (!self.valid || self.backend != Some(backend))
        {
            return Err("Read the current lane's browser with browser_read before using an element number. Its browser or session changed.".into());
        }
        let read = matches!(op, Op::Read | Op::ReadFrames);
        if !matches!(
            op,
            Op::Screenshot | Op::HitTest { .. } | Op::Type { submit: false, .. }
        ) {
            self.invalidate();
        }
        Ok(ReadStamp {
            generation: self.generation,
            backend,
            read,
        })
    }

    /// Failed reads and a stale browser selection never authorize later input.
    pub fn finish(&mut self, stamp: ReadStamp, success: bool) {
        if stamp.read && success && self.generation == stamp.generation {
            self.backend = Some(stamp.backend);
            self.valid = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_browsers_during_a_read_cannot_reauthorize_old_element_numbers() {
        let mut refs = References::default();
        let stamp = refs.begin(Backend::Embedded, &Op::Read).unwrap();
        refs.invalidate();
        refs.finish(stamp, true);
        assert!(
            refs.begin(Backend::Embedded, &Op::Click { target: 1 })
                .is_err()
        );
    }
    #[test]
    fn a_queued_eval_prevents_an_earlier_read_from_reauthorizing_numbers() {
        let mut refs = References::default();
        let stamp = refs.begin(Backend::Embedded, &Op::Read).unwrap();
        refs.begin(
            Backend::Embedded,
            &Op::Eval {
                expression: "1".into(),
                frame: None,
            },
        )
        .unwrap();
        refs.finish(stamp, true);
        assert!(
            refs.begin(Backend::Embedded, &Op::Click { target: 1 })
                .is_err()
        );
    }
    #[test]
    fn a_screenshot_and_hit_test_preserve_read_numbers_but_eval_invalidates_them() {
        let mut refs = References::default();
        let stamp = refs.begin(Backend::Embedded, &Op::ReadFrames).unwrap();
        refs.finish(stamp, true);
        assert!(refs.begin(Backend::Embedded, &Op::Screenshot).is_ok());
        assert!(
            refs.begin(Backend::Embedded, &Op::HitTest { x: 1.0, y: 1.0 })
                .is_ok()
        );
        assert!(
            refs.begin(
                Backend::Embedded,
                &Op::Type {
                    target: 1,
                    text: "x".into(),
                    submit: false
                }
            )
            .is_ok()
        );
        assert!(
            refs.begin(Backend::LocalOya, &Op::Click { target: 1 })
                .is_err()
        );
        refs.begin(
            Backend::Embedded,
            &Op::Eval {
                expression: "1".into(),
                frame: None,
            },
        )
        .unwrap();
        assert!(
            refs.begin(Backend::Embedded, &Op::Click { target: 1 })
                .is_err()
        );
    }
}
