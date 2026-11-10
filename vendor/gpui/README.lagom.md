# Local GPUI patch

Source: gpui 0.2.2 from crates.io, archive SHA-256
`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`.
The library sources, build script, resources and Apache license are retained.
Examples, standalone upstream tests, documentation, their manifest targets
and package author metadata are omitted. This dependency is excluded from workspace membership; Lagom's
headless tests exercise the patch without launching an application.

Local source changes:

- `src/platform.rs`: shared reopen routing, including when another window is
  visible. A visible message popout must not prevent reopening a hidden main.
- `src/platform/mac/platform.rs`: use that routing for the AppKit delegate
  and return the required BOOL (NO when handled, YES for AppKit fallback).
- `src/platform/test/platform.rs`: retain the reopen callback, simulate the
  same visibility input and record platform quit requests.
- `src/app/test_context.rs`: expose those test operations; register reopen
  through `Application::on_reopen`, as production does.

The desktop regressions dispatch Close Window, assert no quit request, and
reopen through the platform callback with a surviving popout. A positive
control checks that `App::quit` is observable. Native AppKit visibility still
requires manual acceptance; headless tests do not call `orderOut`.

Remove the patch when upstream supports both visible-window reopening and
observable quit/reopen test events.
