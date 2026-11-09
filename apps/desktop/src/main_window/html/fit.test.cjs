const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { runInNewContext } = require('node:vm');

function fixture() {
  const events = {};
  const zooms = [];
  const context = {
    document: {
      body: { scrollWidth: 800, style: { setProperty: (key, value, priority) => zooms.push([key, value, priority]) } },
      documentElement: { clientWidth: 400, scrollWidth: 800 },
    },
    addEventListener: (name, callback) => { events[name] = callback; },
    requestAnimationFrame: callback => callback(),
  };
  runInNewContext(readFileSync(`${__dirname}/fit.js`, 'utf8'), context);
  return { context, events, zooms };
}

test('wide mail shrinks; narrow, equal and hidden panes never enlarge it', () => {
  const { context: { fitScale } } = fixture();
  assert.equal(fitScale(400, 800), 0.5);
  assert.equal(fitScale(600, 800), 0.75);
  assert.equal(fitScale(800, 800), 1);
  assert.equal(fitScale(800, 400), 1);
  assert.equal(fitScale(0, 800), 1);
  assert.equal(fitScale(800, 0), 1);
});

test('load, late images and resize measure at original zoom and can restore it', () => {
  const { context, events, zooms } = fixture();
  for (const event of ['DOMContentLoaded', 'load', 'resize']) {
    events[event]();
    assert.deepEqual(zooms.slice(-2), [['zoom', '1', 'important'], ['zoom', '0.5', 'important']]);
  }
  context.document.documentElement.clientWidth = 1000;
  events.resize();
  assert.deepEqual(zooms.at(-1), ['zoom', '1', 'important']);
  context.document.body = null;
  assert.doesNotThrow(events.resize);
});
