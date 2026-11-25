function fitScale(viewport, content) {
  return viewport > 0 && content > viewport ? viewport / content : 1;
}

let sentHeight = -1;
let seen = { width: 0, view: 0, height: 0 };
// Measurements in a row where the content changed with the view's height.
let following = 0;
function fitMail() {
  const body = document.body;
  if (!body) return;
  // Measure without the previous zoom so growing the pane can restore the original size.
  body.style.setProperty('zoom', '1', 'important');
  const root = document.documentElement;
  const scale = fitScale(root.clientWidth, Math.max(root.scrollWidth, body.scrollWidth));
  body.style.setProperty('zoom', String(scale), 'important');
  // The content's height, not the view's: scrollHeight never drops below the view, and a root
  // or body as tall as the view (`height:100%`, `min-height:100vh`) would report it back.
  const view = innerHeight;
  const styles = [root, body].map(node => node.getAttribute('style'));
  for (const node of [root, body]) {
    const style = getComputedStyle(node);
    for (const [key, auto] of [['height', 'auto'], ['min-height', '0']]) {
      if (Math.abs(parseFloat(style.getPropertyValue(key)) - view) <= 1) node.style.setProperty(key, auto, 'important');
    }
  }
  const height = Math.ceil(root.getBoundingClientRect().height);
  [root, body].forEach((node, i) => styles[i] === null ? node.removeAttribute('style') : node.setAttribute('style', styles[i]));
  // The content changing as only the view's height changes follows the view (`100vh`). Once by
  // chance is an image that loaded meanwhile; again and again would grow without end, so it
  // stops growing and the page scrolls the rest itself. A new width measures afresh.
  const width = root.clientWidth;
  if (width !== seen.width) following = 0;
  else if (height !== seen.height) following = view !== seen.view ? following + 1 : 0;
  seen = { width, view, height };
  if (height !== sentHeight && (following < 2 || height < sentHeight)) {
    sentHeight = height;
    window.ipc.postMessage('height ' + height);
  }
}

let fitPending = false;
function scheduleFit() {
  if (fitPending) return;
  fitPending = true;
  requestAnimationFrame(() => { fitPending = false; fitMail(); });
}
addEventListener('DOMContentLoaded', () => {
  scheduleFit();
  // Inline images arrive late; they, fonts and the zoom change the height without a resize.
  new ResizeObserver(scheduleFit).observe(document.documentElement);
});
addEventListener('load', scheduleFit, true);
addEventListener('resize', scheduleFit);

// The view is as tall as the mail, so the reading pane scrolls, not the page. A web view keeps
// wheel and scroll keys, so the app gets those that nothing in the page can use: a scrolling
// box in the mail, or mail over the app's height cap, scrolls first.
function canScroll(node, down) {
  const root = document.documentElement;
  // The root scrolls the view unless the mail hides its overflow.
  const scrolls = node === root
    ? ![root, document.body].some(n => n && /hidden|clip/.test(getComputedStyle(n).overflowY))
    : /auto|scroll/.test(getComputedStyle(node).overflowY);
  return scrolls && (down ? node.scrollTop + node.clientHeight < node.scrollHeight - 1 : node.scrollTop > 0);
}
function scrollPane(e, dy, mode) {
  if (!e.isTrusted || e.composedPath().some(node => node.nodeType === 1 && canScroll(node, dy > 0))) return;
  e.preventDefault();
  window.ipc.postMessage(`wheel ${dy} ${mode}`);
}
addEventListener('wheel', e => {
  if (e.ctrlKey || e.deltaY === 0 || Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
  scrollPane(e, e.deltaY, e.deltaMode);
}, { passive: false });
// [distance, DOM deltaMode]: pixels or pages, down positive.
const scrollKeys = { ' ': [1, 2], PageDown: [1, 2], PageUp: [-1, 2], ArrowDown: [40, 0], ArrowUp: [-40, 0], End: [1e9, 0], Home: [-1e9, 0] };
addEventListener('keydown', e => {
  const step = scrollKeys[e.key];
  // Shift+Space pages up; Shift with other keys extends a selection.
  if (!step || e.metaKey || e.ctrlKey || e.altKey || (e.shiftKey && e.key !== ' ')) return;
  if (e.target.closest?.('input,textarea,select,button,summary,[contenteditable]')) return;
  scrollPane(e, e.shiftKey ? -step[0] : step[0], step[1]);
}, true);
