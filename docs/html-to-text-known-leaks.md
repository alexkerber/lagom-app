# HTML to text: known hidden-text cases

Preview and search text (`mime::html_to_text`) come from a full DOM for every mail. The vendored html5ever tree builder stops at a work limit (`vendor/html5ever/README.lagom.md`). A mail over that limit gets no preview or search text. A sender cannot choose a cheaper, less exact path.

`html_to_text` parses the mail inside the same document start the reader uses (`<!DOCTYPE html><html><head></head>`), after `inert_tags`. A review measured the tree shape against Chrome and WebKit on random tag soup; they matched, except for WebKit's dropped `script` elements.

Still open in previews and search:

- Rules in `<style>` sheets: a class rule can hide text in the reader. lagom reads inline styles only.
- Inline styles with escapes, comments, at-rules or brackets count as hidden, so their text is missing from the preview.
- `body_text` takes a mail's `text/plain` alternative when it has one. The reader never shows that part, so search can find text the reader does not show.

The streaming converter remains for one use: `mime::html_to_blocks`, the native reader on Linux. The inputs below show hidden text there. Previews and search show none of them, with or without padding (`documented_html_leaks`).

| Class | Cause | Inputs |
|---|---|---|
| A | List counters do not drop when a parent's end tag closes the list | `<div><ul></div><li hidden>x</ul>Secret`, `<button><ul></button><li hidden>x</ul>Secret`, `<table><tr><td><ul></td></tr></table><li hidden>x</ul>Secret`, `<dd><dl></dd><dd hidden></dl>Secret` |
| B | Stale svg, select or template state drops a later hidden start tag | `<template><select></template><select hidden></template>Secret`, `<svg><select></svg><select hidden></svg>Secret`, `<svg><summary></svg></summary><summary hidden/></svg>Secret`, `<math><plaintext></math></plaintext><plaintext hidden/></math>Secret` |
| C | Implicit closes that are not modelled leave stale entries | `<div hidden><table><table></table><div></table></div>Secret`, `<div hidden><button><button></button><div></button></div>Secret`, `<span hidden><h1><div hidden></h1><h1></div></span>Secret`, `<table><font hidden/></table><h1 hidden/>w49 <h1></h1></font>Secret` |
| D | `</table>` inside a template lowers the table counter | `<table><tr><td><template></table></template><td hidden>Secret` |
| E | `<form>` in foreign content sets the form pointer | `<svg><td><form></td></svg><form hidden>Secret` |

The November 2026 review of the streaming converter found no panic in about 47 million short random inputs. It found no super-linear path: the worst 1 MB input took 0.24 s.
