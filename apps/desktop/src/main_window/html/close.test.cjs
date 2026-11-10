const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { runInNewContext } = require('node:vm');

test('HTML focus forwards only trusted platform close shortcuts', () => {
  const source = readFileSync(`${__dirname}/../html.rs`, 'utf8').match(/const BRIDGE: &str = r#"([\s\S]*?)"#;/)[1];
  for (const closeModifier of ['metaKey', 'ctrlKey']) {
    const events = {};
    const messages = [];
    runInNewContext(source, {
      closeModifier,
      addEventListener: (name, callback) => { events[name] = callback; },
      window: { ipc: { postMessage: message => messages.push(message) } },
    });
    for (const override of [{}, { isTrusted: false }, { shiftKey: true }, { altKey: true },
      { metaKey: true, ctrlKey: true }, { [closeModifier]: false }, { key: 'q' }]) {
      messages.length = 0;
      let prevented = false;
      const event = { key: 'w', isTrusted: true, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false,
        [closeModifier]: true, target: {}, preventDefault: () => { prevented = true; }, ...override };
      events.keydown(event);
      const expected = Object.keys(override).length === 0;
      assert.equal(messages.includes('close-window'), expected);
      assert.equal(prevented, expected);
    }
  }
});
