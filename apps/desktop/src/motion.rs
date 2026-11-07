//! Easing for motion that moves an object in space (docs/motion.md).

/// Damping ratio and stiffness, per animation duration: overshoot of about 3 % at mid-way,
/// settled to under 0.1 % at the end, so the spring fits the existing 120–200 ms durations.
const ZETA: f32 = 0.75;
const OMEGA: f32 = 9.5;

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
    fn spring_starts_at_0_ends_at_1_and_overshoots_a_little() {
        assert_eq!(spring(0.), 0.);
        assert_eq!(spring(1.), 1.);
        assert!((spring(0.9999) - 1.).abs() < 1e-3, "no jump at the end");
        let samples: Vec<f32> = (0..=1000).map(|i| spring(i as f32 / 1000.)).collect();
        let peak = samples.iter().cloned().fold(0., f32::max);
        assert!(peak > 1.01 && peak < 1.05, "peak {peak}");
        assert!(samples.iter().all(|&x| x >= 0.));
        // Most of the way there in the first fifth, like the ease-out it replaces.
        assert!(spring(0.2) > 0.6);
        // Settled: the last tenth stays within 1 % of the target.
        assert!(samples[900..].iter().all(|&x| (x - 1.).abs() < 0.01));
    }
}
