const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { runInNewContext } = require('node:vm');

function element(fields) {
  const node = { nodeType: 1, attribute: null, overflowY: 'visible', scrollTop: 0, inline: {}, ...fields };
  node.style = { setProperty: (key, value, priority) => { node.inline[key] = [value, priority]; node.onStyle?.(key, value, priority); } };
  node.getAttribute = () => node.attribute;
  node.setAttribute = (_, value) => { node.attribute = value; node.inline = {}; };
  node.removeAttribute = () => { node.attribute = null; node.inline = {}; };
  return node;
}

// The root's layout: `content` px of mail; `full` mail sets `html{height:100%}`; `follows` mail
// has a `100vh` block and a 50 px footer.
function fixture(mode = 'full') {
  const events = {};
  const zooms = [];
  const messages = [];
  const observed = [];
  const body = element({ scrollWidth: 800, attribute: 'margin:0', onStyle: (key, value, priority) => key === 'zoom' && zooms.push([key, value, priority]) });
  const root = element({ clientWidth: 400, scrollWidth: 800, clientHeight: 480, scrollHeight: 480, content: 300.2, mode });
  const context = {
    document: { body, documentElement: root },
    innerHeight: 480,
    window: { ipc: { postMessage: message => messages.push(message) } },
    addEventListener: (name, callback) => { events[name] = callback; },
    requestAnimationFrame: callback => callback(),
    ResizeObserver: class { constructor(callback) { this.callback = callback; } observe(target) { observed.push([target, this.callback]); } },
  };
  const layout = () => root.mode === 'follows' ? context.innerHeight + 50
    : root.mode === 'full' && root.inline.height?.[0] !== 'auto' ? context.innerHeight : root.content;
  context.getComputedStyle = node => ({
    overflowY: node.overflowY,
    getPropertyValue: key => node === root && key === 'height' ? `${layout()}px` : key === 'height' ? '200px' : '0px',
  });
  root.getBoundingClientRect = () => ({ height: layout() });
  runInNewContext(readFileSync(`${__dirname}/fit.js`, 'utf8'), context);
  return { context, events, zooms, messages, observed, root, body };
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

test('the app hears the content height on load and when a late image grows or shrinks the page', () => {
  for (const mode of ['auto', 'full']) {
    const { context, events, messages, observed, root, body } = fixture(mode);
    events.DOMContentLoaded();
    assert.deepEqual(messages, ['height 301'], `${mode}: the content, not a root as tall as the view`);
    assert.equal(root.attribute, null, 'the measuring styles are gone again');
    assert.equal(body.attribute, 'margin:0');
    assert.equal(observed[0][0], root, 'the whole document is watched, so inline images count');
    context.innerHeight = 301;
    events.resize();
    root.content = 1700;
    observed[0][1]();
    events.load();
    root.content = 900;
    events.resize();
    assert.deepEqual(messages, ['height 301', 'height 1700', 'height 900'], `${mode}: one message per change`);
  }
});

test('a first height of zero is reported', () => {
  const { events, messages, root } = fixture('auto');
  root.content = 0;
  events.DOMContentLoaded();
  assert.deepEqual(messages, ['height 0']);
});

test('an image that loads as the view resizes still counts', () => {
  const { context, events, messages, root } = fixture('auto');
  root.content = 600;
  events.DOMContentLoaded();
  context.innerHeight = 600;
  root.content = 720;
  events.resize();
  assert.deepEqual(messages, ['height 600', 'height 720']);
});

test('content that follows the view height stops growing it', () => {
  const { context, events, messages, observed, root } = fixture('follows');
  events.DOMContentLoaded();
  for (const view of [530, 580, 630, 680]) {
    context.innerHeight = view;
    events.resize();
    observed[0][1]();
  }
  assert.deepEqual(messages, ['height 530', 'height 580']);
  root.clientWidth = 380;
  events.resize();
  assert.equal(messages.at(-1), 'height 730', 'a new width measures afresh');
});

function wheelFixture() {
  const f = fixture('auto');
  f.send = (name, override = {}, path = [f.body, f.root]) => {
    let prevented = false;
    f.events[name]({ isTrusted: true, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, deltaX: 0, deltaY: 40, deltaMode: 0,
      target: { closest: () => null }, composedPath: () => [...path, f.context.document, f.context],
      preventDefault: () => { prevented = true; }, ...override });
    return prevented;
  };
  return f;
}

test('a vertical wheel scrolls the reading pane unless something in the page scrolls that way', () => {
  const { send, messages, root, body } = wheelFixture();
  assert.equal(send('wheel'), true);
  assert.equal(send('wheel', { deltaY: -3, deltaMode: 1 }), true);
  assert.equal(send('wheel', { deltaY: 1, deltaMode: 2 }), true);
  assert.deepEqual(messages, ['wheel 40 0', 'wheel -3 1', 'wheel 1 2']);
  for (const override of [{ isTrusted: false }, { ctrlKey: true }, { deltaX: 50 }, { deltaY: 0 }]) assert.equal(send('wheel', override), false);
  // Mail over the height cap scrolls inside the view, and hands on at its ends.
  root.scrollHeight = 20000;
  assert.equal(send('wheel'), false);
  assert.equal(send('wheel', { deltaY: -40 }), true, 'at the top, up goes to the pane');
  root.scrollTop = 20000 - 480;
  assert.equal(send('wheel'), true, 'at the bottom, down goes to the pane');
  root.scrollTop = 0;
  root.overflowY = 'hidden';
  assert.equal(send('wheel'), true, 'a view the mail will not let scroll hands on');
  // A scrolling box in the mail scrolls first.
  root.overflowY = 'visible';
  root.scrollHeight = 480;
  const box = { ...body, nodeType: 1, overflowY: 'auto', scrollTop: 0, clientHeight: 100, scrollHeight: 300 };
  assert.equal(send('wheel', {}, [box, body, root]), false);
  assert.equal(send('wheel', { deltaY: -40 }, [box, body, root]), true);
});

test('scroll keys page the reading pane, except in controls and with modifiers', () => {
  const { send, messages } = wheelFixture();
  for (const [key, shiftKey] of [[' ', false], [' ', true], ['PageDown', false], ['PageUp', false], ['ArrowDown', false], ['End', false], ['Home', false]]) {
    assert.equal(send('keydown', { key, shiftKey }), true, key);
  }
  assert.deepEqual(messages, ['wheel 1 2', 'wheel -1 2', 'wheel 1 2', 'wheel -1 2', 'wheel 40 0', 'wheel 1000000000 0', 'wheel -1000000000 0']);
  let asked = '';
  const control = { closest: selector => { asked = selector; return {}; } };
  for (const override of [{ key: 'a' }, { key: 'ArrowDown', shiftKey: true }, { key: ' ', metaKey: true }, { key: ' ', target: control }]) {
    assert.equal(send('keydown', override), false);
  }
  assert.ok(asked.split(',').includes('summary'), 'Space still opens a disclosure');
});
