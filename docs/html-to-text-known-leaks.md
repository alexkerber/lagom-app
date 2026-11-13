# HTML to text: known hidden-text cases

`crates/lagom-core/src/mime.rs` `html_to_text` is a streaming pass that models the tree builder's closes so it can skip hidden elements. A Claude review on 2026-11-13 found these crafted inputs. Each one prints `Secret`, but the DOM (dom_query 0.28 on the same html5ever 0.39) shows nothing. They affect previews and search text only. Rendering is not affected.

| Class | Cause | Inputs |
|---|---|---|
| A | List counters do not drop when a parent's end tag closes the list | `<div><ul></div><li hidden>x</ul>Secret`, `<button><ul></button><li hidden>x</ul>Secret`, `<table><tr><td><ul></td></tr></table><li hidden>x</ul>Secret`, `<dd><dl></dd><dd hidden></dl>Secret` |
| B | Stale svg, select or template state drops a later hidden start tag | `<template><select></template><select hidden></template>Secret`, `<svg><select></svg><select hidden></svg>Secret`, `<svg><summary></svg></summary><summary hidden/></svg>Secret`, `<math><plaintext></math></plaintext><plaintext hidden/></math>Secret` |
| C | Implicit closes that are not modelled leave stale entries | `<div hidden><table><table></table><div></table></div>Secret`, `<div hidden><button><button></button><div></button></div>Secret`, `<span hidden><h1><div hidden></h1><h1></div></span>Secret`, `<table><font hidden/></table><h1 hidden/>w49 <h1></h1></font>Secret` |
| D | `</table>` inside a template lowers the table counter | `<table><tr><td><template></table></template><td hidden>Secret` |
| E | `<form>` in foreign content sets the form pointer | `<svg><td><form></td></svg><form hidden>Secret` |

The review found no panic in about 47 million short random inputs. It found no super-linear path: the worst 1 MB input took 0.24 s.

Better design (TODO.md): estimate the tree builder's cost in one tokenizer pass. Under a budget, use the DOM, which is exact. Above it, use the streaming or `mail_parser` text.
