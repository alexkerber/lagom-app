# HTML to text: known hidden-text cases

Lagom builds a DOM for preview and search text when the estimated tree-building cost fits 262,144 work units. The estimate counts input bytes, potential stack scans, and formatting elements and attributes that later blocks could recreate. Larger estimates use the streaming converter.

All 14 inputs below produce empty text on the DOM path. **All 14 can still expose `Secret` above budget.** The table describes the streaming fallback.

| Class | Cause | Inputs |
|---|---|---|
| A | List counters do not drop when a parent's end tag closes the list | `<div><ul></div><li hidden>x</ul>Secret`, `<button><ul></button><li hidden>x</ul>Secret`, `<table><tr><td><ul></td></tr></table><li hidden>x</ul>Secret`, `<dd><dl></dd><dd hidden></dl>Secret` |
| B | Stale svg, select or template state drops a later hidden start tag | `<template><select></template><select hidden></template>Secret`, `<svg><select></svg><select hidden></svg>Secret`, `<svg><summary></svg></summary><summary hidden/></svg>Secret`, `<math><plaintext></math></plaintext><plaintext hidden/></math>Secret` |
| C | Implicit closes that are not modelled leave stale entries | `<div hidden><table><table></table><div></table></div>Secret`, `<div hidden><button><button></button><div></button></div>Secret`, `<span hidden><h1><div hidden></h1><h1></div></span>Secret`, `<table><font hidden/></table><h1 hidden/>w49 <h1></h1></font>Secret` |
| D | `</table>` inside a template lowers the table counter | `<table><tr><td><template></table></template><td hidden>Secret` |
| E | `<form>` in foreign content sets the form pointer | `<svg><td><form></td></svg><form hidden>Secret` |

The November 2026 review of the streaming converter found no panic in about 47 million short random inputs. It found no super-linear path: the worst 1 MB input took 0.24 s.
