// Include on an allowed same-site page: <script src="http://localhost:47833/embed.js" defer></script>
(() => {
  const script = document.currentScript;
  const origin = new URL(script.src).origin;
  const frame = document.createElement('iframe');
  frame.src = `${origin}/?widget=1`;
  frame.title = 'Jarvis personal assistant';
  frame.referrerPolicy = 'strict-origin';
  Object.assign(frame.style, { position: 'fixed', right: '12px', bottom: '8px', width: '112px', height: '112px', border: '0', background: 'transparent', zIndex: '2147483000', colorScheme: 'normal' });
  function resize(event) {
    if (event.source !== frame.contentWindow || event.origin !== origin || event.data?.type !== 'jarvis:resize') return;
    frame.style.width = event.data.open ? 'min(440px, 100vw)' : '112px';
    frame.style.height = event.data.open ? 'min(800px, 100dvh)' : '112px';
  }
  window.addEventListener('message', resize);
  document.body.append(frame);
})();
