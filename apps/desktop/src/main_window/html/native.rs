use gpui::{Bounds, Pixels};
use wry::dpi::{LogicalPosition, LogicalSize};

pub(super) struct View {
    web: wry::WebView,
    #[cfg(target_os = "macos")]
    clip: objc2::rc::Retained<objc2_app_kit::NSView>,
}

impl std::ops::Deref for View {
    type Target = wry::WebView;
    fn deref(&self) -> &Self::Target { &self.web }
}

impl View {
    pub(super) fn new(web: wry::WebView) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            let native = web.webview();
            // A force-click preview would fetch the link outside the navigation gate and the CSP.
            let _: () = unsafe { objc2::msg_send![&*native, setAllowsLinkPreview: false] };
            let parent = unsafe { native.superview() }?;
            let clip = objc2_app_kit::NSView::new(objc2::MainThreadMarker::new()?);
            clip.setWantsLayer(true);
            clip.layer()?.setMasksToBounds(true);
            clip.setHidden(true);
            parent.addSubview(&clip);
            clip.addSubview(&native);
            Some(Self { web, clip })
        }
        #[cfg(target_os = "windows")]
        Some(Self { web })
    }

    /// The page as it looks now, as PNG, through `done` on the main thread. GPUI draws it while an
    /// overlay hides the view. `None` where the platform has no snapshot yet (Windows).
    pub(super) fn snapshot(&self, done: impl FnOnce(Option<Vec<u8>>) + 'static) {
        #[cfg(target_os = "macos")]
        {
            use objc2::{msg_send, rc::Retained, runtime::AnyObject};
            use objc2_app_kit::NSImage;
            use objc2_foundation::{NSData, NSError};
            use wry::WebViewExtMacOS;
            fn png(image: &NSImage) -> Option<Vec<u8>> {
                let tiff: Option<Retained<NSData>> = unsafe { msg_send![image, TIFFRepresentation] };
                let rep: Option<Retained<AnyObject>> = unsafe { msg_send![objc2::class!(NSBitmapImageRep), imageRepWithData: &*tiff?] };
                let properties: Retained<AnyObject> = unsafe { msg_send![objc2::class!(NSDictionary), new] };
                // 4 is NSBitmapImageFileTypePNG.
                let data: Option<Retained<NSData>> = unsafe { msg_send![&*rep?, representationUsingType: 4usize, properties: &*properties] };
                Some(data?.to_vec())
            }
            let done = std::cell::Cell::new(Some(done));
            let block = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
                if let Some(done) = done.take() { done(unsafe { image.as_ref() }.and_then(png)); }
            });
            let native = self.web.webview();
            let _: () = unsafe { msg_send![&*native, takeSnapshotWithConfiguration: std::ptr::null::<AnyObject>(), completionHandler: &*block] };
        }
        #[cfg(target_os = "windows")]
        done(None);
    }

    pub(super) fn set_visible(&self, visible: bool) -> wry::Result<()> {
        #[cfg(target_os = "macos")]
        self.clip.setHidden(!visible);
        self.web.set_visible(visible)
    }

    pub(super) fn place(&self, bounds: Bounds<Pixels>, clip: Bounds<Pixels>, _scale: f32) -> wry::Result<()> {
        #[cfg(target_os = "windows")]
        let origin = bounds.origin;
        #[cfg(target_os = "macos")]
        let origin = {
            use objc2_foundation::{NSPoint, NSRect, NSSize};
            let parent = unsafe { self.clip.superview() }.expect("attached clip view");
            let x = f64::from(f32::from(clip.origin.x));
            let mut y = f64::from(f32::from(clip.origin.y));
            let width = f64::from(f32::from(clip.size.width));
            let height = f64::from(f32::from(clip.size.height));
            if !parent.isFlipped() { y = parent.bounds().size.height - y - height; }
            self.clip.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            bounds.origin - clip.origin
        };
        self.web.set_bounds(wry::Rect {
            position: LogicalPosition::new(f64::from(f32::from(origin.x)), f64::from(f32::from(origin.y))).into(),
            size: LogicalSize::new(f64::from(f32::from(bounds.size.width)), f64::from(f32::from(bounds.size.height))).into(),
        })?;
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
            use wry::WebViewExtWindows;
            let offset = clip.origin - bounds.origin;
            let pixel = |value: Pixels| (f32::from(value) * _scale).round() as i32;
            unsafe {
                let region = CreateRectRgn(pixel(offset.x), pixel(offset.y), pixel(offset.x + clip.size.width), pixel(offset.y + clip.size.height));
                if region.is_invalid() { return Err(std::io::Error::last_os_error().into()); }
                // Windows owns the region only after a successful SetWindowRgn.
                if SetWindowRgn(windows::Win32::Foundation::HWND(self.web.hwnd().0), Some(region), true) == 0 {
                    let error = std::io::Error::last_os_error();
                    let _ = DeleteObject(region.into());
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for View {
    fn drop(&mut self) { self.clip.removeFromSuperview(); }
}
