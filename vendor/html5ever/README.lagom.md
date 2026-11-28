# Local html5ever patch

Source: html5ever 0.39.0 from crates.io, archive SHA-256
`46a1761807faccc9a19e86944bbf40610014066306f96edcdedc2fb714bcb7b8`.
The original sources and licenses are retained. The manifest drops the
examples, tests and benches, which are not copied.

Two changes. First, `Tokenizer::finish_attribute` in `src/tokenizer/mod.rs`: a tag
keeps its first 256 attributes and drops the rest, except `hidden`, `style`, `open`,
`name`, `popover` and `shadowrootmode`. lagom-core reads those to skip hidden text, so a
tag keeps at most 262 attributes. Upstream checks each new
attribute against every earlier one, so one tag with k distinct attributes costs
k² comparisons. Measured before the patch: a single tag with 80,000 attributes
(550 KB of mail HTML) took 20 s to tokenize at sync.

A pre-scan in lagom-core is not enough: it would have to track quotes, comments
and raw text exactly as the tokenizer does, or an attacker hides a large tag in a
region the scan reads differently.

Second, the tree builder (`src/tree_builder/mod.rs`, a few calls in `rules.rs`) counts
work and stops at a limit. Each token costs one unit plus the depth of the stack of open
elements plus the length of the active formatting list. Lookups in either list, removals
from the stack, new elements (160 units plus their attribute bytes), comments (320),
parse errors (8), attribute copies and compares, and attribute merges into a repeated
`html` or `body` add their own cost. Parse errors are counted, not passed to the sink:
a sink that keeps each one grows with the input.

The limit is `WORK_LIMIT` (2^24 units), or less after `set_work_limit` on the same
thread. lagom-core sets 32 units per input byte plus 65,536, so a mail of many HTML parts
stays linear in its size. Past the limit, the builder ignores every later token and
reports the parse error `WORK_LIMIT_ERROR` once. lagom-core then gives that part no text:
a partial tree can show text that the whole document hides. Upstream, deep nesting,
unclosed formatting elements and the adoption agency make some inputs quadratic in time
and in nodes.

Measured (release, MacBook): 2,527 real HTML mails need at most 8.8 units per input
byte and at most 20 % of their limit. The slowest took 8 ms. Every input in
`html_bombs_parse_in_linear_time` stops or finishes within 0.7 s per call, at most about
40 MB above the test's own allocations.

The crate is excluded from the workspace, so its optional features stay out of
`Cargo.lock`. `mime::tests::html_bombs_parse_in_linear_time` in lagom-core
and `html_text_matches_the_dom_converter` cover the patch.

Remove the tokenizer change when upstream checks duplicates in constant time or
bounds attributes per tag. Remove the tree builder change when upstream bounds its
work per document.
