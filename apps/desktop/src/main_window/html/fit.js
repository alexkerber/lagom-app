function fitScale(viewport, content) {
  return viewport > 0 && content > viewport ? viewport / content : 1;
}

function fitMail() {
  const body = document.body;
  if (!body) return;
  // Measure without the previous zoom so growing the pane can restore the original size.
  body.style.setProperty('zoom', '1', 'important');
  const root = document.documentElement;
  const scale = fitScale(root.clientWidth, Math.max(root.scrollWidth, body.scrollWidth));
  body.style.setProperty('zoom', String(scale), 'important');
}

let fitPending = false;
function scheduleFit() {
  if (fitPending) return;
  fitPending = true;
  requestAnimationFrame(() => { fitPending = false; fitMail(); });
}
addEventListener('DOMContentLoaded', scheduleFit);
addEventListener('load', scheduleFit, true);
addEventListener('resize', scheduleFit);
