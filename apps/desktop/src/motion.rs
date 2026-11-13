//! Easing for motion that moves an object in space, and the system "reduce motion" setting
//! (docs/motion.md).

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A loading note waits this long before it fades in, so fast loads never flash it.
pub const CALM: Duration = Duration::from_millis(150);

/// The system asks for less motion (docs/motion.md rule 6): moving things then only fade.
/// `LAGOM_REDUCE_MOTION=1` (or `0`) overrides the system on every platform.
// shortcut: Windows (SPI_GETCLIENTAREAANIMATION) and Linux (gtk-enable-animations) only read the
// environment override; read their settings when those builds ship to users.
pub fn reduced() -> bool {
    #[cfg(test)]
    if let Some(on) = FORCE.with(|f| f.get()) {
        return on;
    }
    // Read at most once a second: it is asked per row per frame, and changes rarely.
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    match *last {
        Some((at, on)) if at.elapsed() < Duration::from_secs(1) => on,
        _ => {
            let on = read_reduced();
            *last = Some((Instant::now(), on));
            on
        }
    }
}

fn read_reduced() -> bool {
    match std::env::var("LAGOM_REDUCE_MOTION").as_deref() {
        Ok("1") => return true,
        Ok("0") => return false,
        _ => {}
    }
    // Tests stay deterministic; they force the setting with `force_reduced`.
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject}};
        let Some(class) = AnyClass::get(c"NSWorkspace") else { return false };
        // SAFETY: documented NSWorkspace class method and BOOL property, AppKit is linked.
        let workspace: Option<Retained<AnyObject>> = unsafe { msg_send![class, sharedWorkspace] };
        return workspace.is_some_and(|w| unsafe { msg_send![&*w, accessibilityDisplayShouldReduceMotion] });
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(test)]
thread_local!(static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) });

/// Tests: pretend the system setting is `on` on this thread.
#[cfg(test)]
pub fn force_reduced(on: Option<bool>) {
    FORCE.with(|f| f.set(on));
}

/// A distance an object travels, or 0 under reduced motion, where it only fades.
pub fn travel(px: f32) -> f32 {
    if reduced() { 0. } else { px }
}

/// Opacity of a loading note `d` (0..=1) into an animation twice `CALM` long: hidden for the
/// first half, then fades in. A load that finishes sooner never shows it.
pub fn calm(d: f32) -> f32 {
    (d * 2. - 1.).clamp(0., 1.)
}

/// Damping ratio and stiffness, per animation duration: under 1 % overshoot, and the motion
/// spreads over the whole duration (40 % at a quarter, 87 % at a half). The old 0.75/9.5 was done
/// after a third, so at 60 Hz a row jumped in 2–4 frames.
const ZETA: f32 = 0.85;
const OMEGA: f32 = 5.5;

/// A spring released at 0 that settles on 1. `t` is linear progress, 0..=1.
/// Use it inside the animator of a linear `Animation`: GPUI's `with_easing` asserts the eased
/// value stays in 0..=1, and the overshoot leaves that range.
pub fn spring(t: f32) -> f32 {
    if t >= 1. {
        return 1.; // Exactly, so a finished motion sits at rest.
    }
    let t = t.max(0.);
    let root = (1. - ZETA * ZETA).sqrt();
    let x = |t: f32| {
        let wd = OMEGA * root * t;
        1. - (-ZETA * OMEGA * t).exp() * (wd.cos() + ZETA / root * wd.sin())
    };
    // The undamped remainder at t = 1 is under 0.1 %; spread it so the spring lands exactly on 1.
    x(t) + (1. - x(1.)) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_starts_at_0_ends_at_1_and_plays_out() {
        assert_eq!(spring(0.), 0.);
        assert_eq!(spring(1.), 1.);
        assert!((spring(0.9999) - 1.).abs() < 1e-3, "no jump at the end");
        let samples: Vec<f32> = (0..=1000).map(|i| spring(i as f32 / 1000.)).collect();
        let peak = samples.iter().cloned().fold(0., f32::max);
        assert!(peak < 1.01, "peak {peak}");
        assert!(samples.iter().all(|&x| x >= 0.));
        // The motion spreads over the duration instead of jumping in the first frames.
        assert!((0.3..0.5).contains(&spring(0.25)), "quarter {}", spring(0.25));
        assert!(spring(0.5) > 0.8);
        // Settled: the last tenth stays within 1 % of the target.
        assert!(samples[900..].iter().all(|&x| (x - 1.).abs() < 0.01));
    }

    #[test]
    fn reduced_motion_removes_travel_and_loading_waits() {
        force_reduced(Some(true));
        assert!(reduced());
        assert_eq!(travel(12.), 0.);
        force_reduced(Some(false));
        assert_eq!(travel(12.), 12.);
        force_reduced(None);
        // Hidden until CALM has passed, then a fade.
        assert_eq!(calm(0.), 0.);
        assert_eq!(calm(0.5), 0.);
        assert!((calm(0.75) - 0.5).abs() < 1e-6);
        assert_eq!(calm(1.), 1.);
    }
}
