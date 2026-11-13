# Local html5ever patch

Source: html5ever 0.39.0 from crates.io, archive SHA-256
`46a1761807faccc9a19e86944bbf40610014066306f96edcdedc2fb714bcb7b8`.
The original sources and licenses are retained. The manifest drops the
examples, tests and benches, which are not copied.

Only `Tokenizer::finish_attribute` in `src/tokenizer/mod.rs` is modified: a tag
keeps its first 256 attributes and drops the rest, except `hidden` and `style`.
lagom-core reads those two to skip hidden text, so a tag keeps at most 258
attributes. Upstream checks each new
attribute against every earlier one, so one tag with k distinct attributes costs
k² comparisons. Measured before the patch: a single tag with 80,000 attributes
(550 KB of mail HTML) took 20 s to tokenize at sync.

A pre-scan in lagom-core is not enough: it would have to track quotes, comments
and raw text exactly as the tokenizer does, or an attacker hides a large tag in a
region the scan reads differently.

The crate is excluded from the workspace, so its optional features stay out of
`Cargo.lock`. `mime::tests::html_bombs_parse_in_linear_time` in lagom-core
and `html_text_matches_the_dom_converter` cover the patch.

Remove this patch when the upstream tokenizer checks duplicates in constant time
or bounds attributes per tag.
