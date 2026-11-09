use gpui::{prelude::*, Rgba, Window};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Backend { Opaque, Glass, VisualEffect }

fn backend(macos: bool, glass_class: bool, installed: bool) -> Backend {
    if !macos || !installed { Backend::Opaque }
    else if glass_class { Backend::Glass }
    else { Backend::VisualEffect }
}

impl Backend {
    fn alpha(self) -> f32 { if self == Self::Opaque { 1. } else { 0.55 } }
}

#[derive(Clone, Default)]
pub(super) struct Glass {
    #[cfg(target_os = "macos")]
    view: Option<std::rc::Rc<native::View>>,
}

impl Glass {
    pub(super) fn new(_window: &Window) -> Self {
        // TestWindow has no native handle; tests exercise the decision with explicit platform facts.
        #[cfg(all(target_os = "macos", not(test)))]
        return Self { view: native::View::new(_window).map(std::rc::Rc::new) };
        #[cfg(any(not(target_os = "macos"), test))]
        Self::default()
    }

    pub(super) fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(view) = &self.view { view.hide(); }
    }

    pub(super) fn background(&self, frost: Rgba) -> impl IntoElement {
        let glass = self.clone();
        gpui::canvas(move |_bounds, _, _| {
            #[cfg(target_os = "macos")]
            if let Some(view) = &glass.view { return backend(true, view.glass, view.place(_bounds)); }
            let _ = glass;
            // shortcut: Windows stays opaque, add Mica/Acrylic when a native backdrop is available.
            backend(cfg!(target_os = "macos"), false, false)
        }, move |bounds, backend, window, _| {
            window.paint_quad(gpui::fill(bounds, Rgba { a: backend.alpha(), ..frost }));
        }).absolute().inset_0()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use gpui::{Bounds, Pixels, Window};
    use objc2::{msg_send, rc::Retained, runtime::AnyClass, MainThreadMarker};
    use objc2_app_kit::{NSGlassEffectView, NSView, NSVisualEffectView, NSVisualEffectMaterial,
        NSVisualEffectBlendingMode, NSVisualEffectState, NSWindowOrderingMode};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub(super) struct View {
        view: Retained<NSView>,
        content: Retained<NSView>,
        pub(super) glass: bool,
    }

    impl View {
        #[cfg_attr(test, allow(dead_code))]
        pub(super) fn new(window: &Window) -> Option<Self> {
            MainThreadMarker::new()?;
            let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None; };
            // GPUI owns this NSView; retaining it keeps the handle valid until the backdrop is removed.
            let content = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }?;
            content.window()?;
            let parent = unsafe { content.superview() }?;
            let class = AnyClass::get(c"NSGlassEffectView");
            let selected = super::backend(true, class.is_some(), true);
            let view: Retained<NSView> = match selected {
                super::Backend::Glass => {
                    let class = class?;
                    let view: Option<Retained<NSGlassEffectView>> = unsafe { msg_send![class, new] };
                    let view = view?;
                    view.setCornerRadius(0.);
                    Retained::into_super(view)
                }
                super::Backend::VisualEffect => {
                    let class = AnyClass::get(c"NSVisualEffectView")?;
                    let view: Option<Retained<NSVisualEffectView>> = unsafe { msg_send![class, new] };
                    let view = view?;
                    view.setMaterial(NSVisualEffectMaterial::Sidebar);
                    view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                    view.setState(NSVisualEffectState::FollowsWindowActiveState);
                    Retained::into_super(view)
                }
                super::Backend::Opaque => return None,
            };
            view.setHidden(true);
            // shortcut: GPUI paints above a sibling glass view, host its sidebar in contentView when it supports separate surfaces.
            parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, Some(&content));
            Some(Self { view, content, glass: selected == super::Backend::Glass })
        }

        pub(super) fn hide(&self) { self.view.setHidden(true); }

        pub(super) fn place(&self, bounds: Bounds<Pixels>) -> bool {
            let Some(parent) = (unsafe { self.view.superview() }) else { return false; };
            if self.content.window().is_none() || unsafe { self.content.superview() }.as_deref() != Some(&*parent) {
                self.hide();
                return false;
            }
            let rect = local_rect(bounds, self.content.bounds().size.height, self.content.isFlipped());
            let frame = self.content.convertRect_toView(rect, Some(&parent));
            if self.view.frame() != frame { self.view.setFrame(frame); }
            self.view.setHidden(false);
            true
        }
    }

    fn local_rect(bounds: Bounds<Pixels>, content_height: f64, flipped: bool) -> NSRect {
        let x = f64::from(f32::from(bounds.origin.x));
        let y = f64::from(f32::from(bounds.origin.y));
        let width = f64::from(f32::from(bounds.size.width));
        let height = f64::from(f32::from(bounds.size.height));
        NSRect::new(NSPoint::new(x, if flipped { y } else { content_height - y - height }), NSSize::new(width, height))
    }

    #[test]
    fn sidebar_frame_uses_logical_layout_bounds_in_either_coordinate_system() {
        use gpui::{point, px, size};
        for width in [180., 220., 320.] {
            let bounds = Bounds::new(point(px(12.), px(8.)), size(px(width), px(600.)));
            for flipped in [false, true] {
                let frame = local_rect(bounds, 900., flipped);
                assert_eq!(frame.origin, NSPoint::new(12., if flipped { 8. } else { 292. }));
                assert_eq!(frame.size, NSSize::new(f64::from(width), 600.));
            }
        }
    }

    impl Drop for View {
        fn drop(&mut self) { self.view.removeFromSuperview(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_and_native_failure_choose_an_opaque_fallback() {
        for macos in [false, true] {
            for glass_class in [false, true] {
                for installed in [false, true] {
                    let selected = backend(macos, glass_class, installed);
                    if !macos || !installed {
                        assert_eq!(selected, Backend::Opaque);
                        assert_eq!(selected.alpha(), 1.);
                    } else {
                        assert_eq!(selected, if glass_class { Backend::Glass } else { Backend::VisualEffect });
                        assert_eq!(selected.alpha(), 0.55);
                    }
                }
            }
        }
    }
}
