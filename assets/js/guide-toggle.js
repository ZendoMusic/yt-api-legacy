/**
 * Toggle guide (sidebar) by button. Works in IE8+ (Windows 7).
 * Uses only var, attachEvent/addEventListener, no classList, no preventDefault in IE path.
 */
(function () {
  function run() {
    var btn = document.getElementById('appbar-guide-button');
    var body = document.body;
    if (!btn || !body) return;

    function toggle() {
      var cls = body.className || '';
      if (cls.indexOf('guide-closed') !== -1) {
        body.className = cls.replace(/\s*guide-closed\s*/g, ' ').replace(/^\s+|\s+$/g, '');
      } else {
        body.className = (cls + ' guide-closed').replace(/^\s+|\s+$/g, ' ');
      }
    }

    function onBtnClick(e) {
      if (e && e.preventDefault) e.preventDefault();
      else if (window.event) window.event.returnValue = false;
      toggle();
      return false;
    }

    if (btn.attachEvent) {
      btn.attachEvent('onclick', onBtnClick);
    } else if (btn.addEventListener) {
      btn.addEventListener('click', onBtnClick, false);
    }
  }

  if (document.readyState === 'complete' || document.readyState === 'loaded') {
    run();
  } else if (document.attachEvent) {
    document.attachEvent('onreadystatechange', function () {
      if (document.readyState === 'complete') run();
    });
  } else {
    document.addEventListener('DOMContentLoaded', run, false);
  }
})();
