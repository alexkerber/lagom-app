# Local imap-proto patch

Source: imap-proto 0.17.0 from crates.io, archive SHA-256
`5ccf963d57074747b455398a1763d174da80bcaba6f51e3671a82252b531a68b`.
The original sources, manifest and licenses are retained.

Only `src/rfc3501/body_structure.rs` is modified: BODYSTRUCTURE and body
extension lists share a recursion budget of 40. Exhaustion returns a fatal
parser failure so optional fields and unknown FETCH attributes cannot swallow
the rejection. The budget belongs to each parse call and is shared down the
tree, not across siblings or responses.

The wire scanner alone is insufficient: unknown FETCH attributes can consume
quotes differently from the scanner, exposing recursive parsing even on
incomplete input. Tests cover this through async-imap in lagom-core, plus
native parser depth boundaries, mixed body/extension nesting and siblings.
This crate is a workspace member so its tests run in workspace CI.

Remove this patch when the upstream parser provides a recursion bound.
