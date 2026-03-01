var playPauseBtn = document.querySelector(".play-pause-btn");
var theaterBtn = document.querySelector(".theater-btn");
var fullScreenBtn = document.querySelector(".full-screen-btn");
var miniPlayerBtn = document.querySelector(".mini-player-btn");
var muteBtn = document.querySelector(".mute-btn");
var captionsBtn = document.querySelector(".captions-btn");
var settingsBtn = document.querySelector(".settings-btn");
var settingsPanel = document.querySelector(".settings-panel");
var currentTimeElem = document.querySelector(".current-time");
var totalTimeElem = document.querySelector(".total-time");
var volumeSlider = document.querySelector(".volume-slider");
var videoContainer = document.querySelector(".video-container");
var timelineContainer = document.querySelector(".timeline-container");
var video = document.querySelector("video");

window.isEventInsidePlayer = function (e) {
  var target = e.target || e.srcElement;
  if (!target || !videoContainer) return false;
  if (videoContainer.contains) return videoContainer.contains(target);
  while (target) {
    if (target === videoContainer) return true;
    target = target.parentNode;
  }
  return false;
};

/* ===========================
   DURATION FROM HEADERS (for IE / streaming when video.duration is NaN)
=========================== */

var durationFromHeader = NaN;

function getDuration() {
  if (durationFromHeader >= 0 && !isNaN(durationFromHeader)) return durationFromHeader;
  var d = video.duration;
  return (d >= 0 && !isNaN(d)) ? d : durationFromHeader;
}

function fetchDurationFromHeaders(callback) {
  var src = (video.src || video.getAttribute("src") || "").trim();
  if (!src) {
    if (callback) callback();
    return;
  }
  var xhr;
  if (window.XMLHttpRequest) {
    xhr = new XMLHttpRequest();
  } else if (window.ActiveXObject) {
    try { xhr = new ActiveXObject("Microsoft.XMLHTTP"); } catch (e) { if (callback) callback(); return; }
  } else {
    if (callback) callback();
    return;
  }
  xhr.open("HEAD", src, true);
  xhr.onreadystatechange = function () {
    if (xhr.readyState !== 4) return;
    var h = (xhr.getResponseHeader && (xhr.getResponseHeader("X-Content-Duration") || xhr.getResponseHeader("Content-Duration") || xhr.getResponseHeader("X-Duration-Seconds"))) || null;
    if (h) {
      var sec = parseInt(h, 10);
      if (!isNaN(sec) && sec >= 0) {
        durationFromHeader = sec;
        if (totalTimeElem) totalTimeElem.innerHTML = formatDuration(sec);
      }
    }
    if (callback) callback();
  };
  try { xhr.send(null); } catch (e) { if (callback) callback(); }
}

/* ===========================
   INITIAL TIME
=========================== */

currentTimeElem.innerHTML = "0:00";
totalTimeElem.innerHTML = "0:00";

/* Fetch duration from response headers (same-origin) so it works in old IE and when video.duration is not available */
fetchDurationFromHeaders();

/* ===========================
   AUTOPLAY
=========================== */

// Attempt autoplay when page loads
window.addEventListener('load', function() {
  // Check if autoplay is allowed by browser policies
  var playPromise = video.play();
  
  if (playPromise !== undefined) {
    playPromise
      .then(function() {
        // Autoplay successful
        console.log("Autoplay successful");
      })
      .catch(function(error) {
        // Autoplay blocked by browser
        console.log("Autoplay blocked:", error);
        // Show play button since autoplay failed
        videoContainer.classList.add("paused");
      });
  }
});

/* ===========================
   FULLSCREEN HELPERS
=========================== */


var isScrubbing = false;
var wasPaused = false;

function getFullscreenElement() {
  return (
    document.fullscreenElement ||
    document.webkitFullscreenElement ||
    document.mozFullScreenElement ||
    document.msFullscreenElement ||
    null
  );
}

function requestFullscreen(el) {
  if (el.requestFullscreen) return el.requestFullscreen();
  if (el.webkitRequestFullscreen) return el.webkitRequestFullscreen();
  if (el.mozRequestFullScreen) return el.mozRequestFullScreen();
  if (el.msRequestFullscreen) return el.msRequestFullscreen();
}

function exitNativeFullscreen() {
  if (document.exitFullscreen) return document.exitFullscreen();
  if (document.webkitExitFullscreen) return document.webkitExitFullscreen();
  if (document.mozCancelFullScreen) return document.mozCancelFullScreen();
  if (document.msExitFullscreen) return document.msExitFullscreen();
}

function isInFullscreen() {
  return (
    getFullscreenElement() ||
    document.documentElement.className.indexOf("ie-fullscreen") !== -1
  );
}

/** IE8-safe trim (String.prototype.trim not in IE8) */
function strTrim(s) {
  return s ? s.replace(/^\s+|\s+$/g, "") : s;
}

/* ===========================
   ESC EMULATION
=========================== */

function simulateEscapeKey() {
  var evt = document.createEvent("Event");
  evt.initEvent("keydown", true, true);
  evt.keyCode = 27;
  evt.which = 27;
  evt.key = "Escape";
  document.dispatchEvent(evt);
}

/* ===========================
   IE9 FALLBACK
=========================== */

function enterIEFullscreen() {
  var html = document.documentElement;
  var body = document.body;

  if (html.className.indexOf("ie-fullscreen") === -1)
    html.className += " ie-fullscreen";
  if (body.className.indexOf("ie-fullscreen") === -1)
    body.className += " ie-fullscreen";
  if (videoContainer.className.indexOf("full-screen") === -1)
    videoContainer.className += " full-screen";

  html.style.overflow = "hidden";
  body.style.overflow = "hidden";
  body.style.position = "fixed";
  body.style.width = "100%";
  body.style.height = "100%";
}

function exitIEFullscreen() {
  var html = document.documentElement;
  var body = document.body;

  html.className = strTrim(html.className.replace(/\bie-fullscreen\b/g, ""));
  body.className = strTrim(body.className.replace(/\bie-fullscreen\b/g, ""));
  videoContainer.className = strTrim(videoContainer.className.replace(/\bfull-screen\b/g, ""));

  html.style.overflow = "";
  body.style.overflow = "";
  body.style.position = "";
  body.style.width = "";
  body.style.height = "";
}

/* ===========================
   FULLSCREEN TOGGLE (FIXED)
=========================== */

function toggleFullScreenMode() {
  if (isInFullscreen()) {
    try { simulateEscapeKey(); } catch (e) {}
    exitNativeFullscreen();
    exitIEFullscreen();
  } else {
    try {
      requestFullscreen(videoContainer);
    } catch (e) {
      enterIEFullscreen();
      return;
    }
    /* Old IE: requestFullscreen does nothing and doesn't throw. After a tick, if still not fullscreen, use IE fallback. */
    setTimeout(function () {
      if (!isInFullscreen()) {
        enterIEFullscreen();
      }
    }, 100);
  }
}

if (fullScreenBtn) {
  if (fullScreenBtn.attachEvent) {
    fullScreenBtn.attachEvent("onclick", toggleFullScreenMode);
  } else {
    fullScreenBtn.addEventListener("click", toggleFullScreenMode);
  }
}

/* ===========================
   SYNC FULLSCREEN CLASS
=========================== */

document.addEventListener("fullscreenchange", syncFullscreenClass);
document.addEventListener("webkitfullscreenchange", syncFullscreenClass);
document.addEventListener("mozfullscreenchange", syncFullscreenClass);
document.addEventListener("MSFullscreenChange", syncFullscreenClass);

function syncFullscreenClass() {
  if (getFullscreenElement()) {
    if (videoContainer.className.indexOf("full-screen") === -1)
      videoContainer.className += " full-screen";
  } else {
    videoContainer.className = strTrim(videoContainer.className.replace(/\bfull-screen\b/g, ""));
  }
}

/* ===========================
   ESC KEY
=========================== */

document.addEventListener("keydown", function (e) {
  if ((e.key === "Escape" || e.keyCode === 27) && isInFullscreen()) {
    exitNativeFullscreen();
    exitIEFullscreen();
  }
});

/* ===========================
   KEYBOARD CONTROLS
=========================== */

/** IE-safe: true if focus is in an input/textarea/select (e.g. search box) — тогда не управлять плеером с клавиатуры */
function isFocusInFormControl() {
  var el = document.activeElement;
  if (!el || !el.tagName) return false;
  var tag = (el.tagName + "").toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (el.contentEditable === "true" || el.contentEditable === true) return true;
  return false;
}

document.addEventListener("keydown", function (e) {
  if (isFocusInFormControl()) return;

  // F — fullscreen: симулируем клик по кнопке плеера (в старом IE вызов из keydown даёт чёрный экран, клик по кнопке — нет)
  if (e.key === "f" || e.key === "F" || e.keyCode === 70) {
    e.preventDefault();
    var img = fullScreenBtn.querySelector(isInFullscreen() ? 'img[alt="Exit fullscreen"]' : 'img[alt="Enter fullscreen"]');
    setTimeout(function () { (img || fullScreenBtn).click(); }, 0);
    return;
  }

  // Prevent default behavior for handled keys
  if ([32, 37, 39].indexOf(e.keyCode) !== -1) {
    e.preventDefault();
  }
  
  // Spacebar - Play/Pause
  if (e.key === " " || e.keyCode === 32) {
    togglePlay();
  }
  
  // Left Arrow - Seek backward 10 seconds
  if (e.key === "ArrowLeft" || e.keyCode === 37) {
    if (video.currentTime >= 10) {
      video.currentTime -= 10;
    } else {
      video.currentTime = 0;
    }
  }
  
  // Right Arrow - Seek forward 10 seconds
  var dur = getDuration();
  if (e.key === "ArrowRight" || e.keyCode === 39) {
    if (dur >= 0 && !isNaN(dur)) {
      if (video.currentTime <= dur - 10) {
        video.currentTime += 10;
      } else {
        video.currentTime = dur;
      }
    }
  }
});

/* ===========================
   TIMELINE / SCRUBBING
=========================== */

var isScrubbing = false;
var wasPaused = false;

timelineContainer.addEventListener("mousedown", function (e) {
  startScrubbing(e);
  showControls();
  clearInactivityTimer();
});
document.addEventListener("mousemove", scrubbingMove);
document.addEventListener("mouseup", stopScrubbing);

function startScrubbing(e) {
  var d = getDuration();
  if (d < 0 || isNaN(d)) return;

  isScrubbing = true;
  wasPaused = video.paused;
  video.pause();
  videoContainer.className += " scrubbing";
  updateTimeline(e);
}

function scrubbingMove(e) {
  if (!isScrubbing) return;
  updateTimeline(e);
}

function stopScrubbing(e) {
  if (!isScrubbing) return;

  isScrubbing = false;
  updateTimeline(e);
  var d = getDuration();
  if (d >= 0 && !isNaN(d)) video.currentTime = getPercent(e) * d;

  if (!wasPaused) video.play();
  videoContainer.className = videoContainer.className.replace(/\bscrubbing\b/g, "").trim();
}

function getPercent(e) {
  var rect = timelineContainer.getBoundingClientRect();
  return Math.min(Math.max(0, e.clientX - rect.left), rect.width) / rect.width;
}

function updateTimeline(e) {
  var percent = getPercent(e);
  var timeline = timelineContainer.querySelector(".timeline");
  var thumb = timelineContainer.querySelector(".thumb-indicator");

  if (timeline)
    timeline.style.background =
      "linear-gradient(to right, red " +
      percent * 100 +
      "%, rgba(100,100,100,.5) " +
      percent * 100 +
      "%)";

  if (thumb) thumb.style.left = percent * 100 + "%";
}

/* ===========================
   TIME UPDATE
=========================== */

video.addEventListener("loadedmetadata", function () {
  var d = getDuration();
  if (d >= 0 && !isNaN(d)) totalTimeElem.innerHTML = formatDuration(d);
});

video.addEventListener("timeupdate", function () {
  var d = getDuration();
  if (d < 0 || isNaN(d)) return;

  currentTimeElem.innerHTML = formatDuration(video.currentTime);

  if (isScrubbing) return;

  var percent = d > 0 ? video.currentTime / d : 0;
  var timeline = timelineContainer.querySelector(".timeline");
  var thumb = timelineContainer.querySelector(".thumb-indicator");

  if (timeline)
    timeline.style.background =
      "linear-gradient(to right, red " +
      percent * 100 +
      "%, rgba(100,100,100,.5) " +
      percent * 100 +
      "%)";

  if (thumb) thumb.style.left = percent * 100 + "%";
});

function formatDuration(time) {
  if (!time || isNaN(time)) return "0:00";
  var s = Math.floor(time % 60);
  var m = Math.floor(time / 60);
  return m + ":" + (s < 10 ? "0" + s : s);
}

/* ===========================
   PLAY / PAUSE
=========================== */

playPauseBtn.addEventListener("click", togglePlay);
video.addEventListener("click", handleVideoTap);

function togglePlay() {
  if (!canTogglePlayback()) return;

  if (video.paused) {
    var playPromise = video.play();
    if (playPromise !== undefined && typeof playPromise.catch === "function") {
      playPromise.catch(function (error) {
        console.error("Play promise failed:", error);
        showControls();
      });
    }
  } else {
    video.pause();
  }
}

function handleVideoTap(e) {
  if (isTouchDevice()) {
    handleUserActivity();
    e.stopPropagation();
    e.preventDefault();
    return;
  }

  if (hasClass(videoContainer, "hide-controls")) {
    handleUserActivity();
    e.stopPropagation();
    e.preventDefault();
    return;
  }

  togglePlay();
}

video.addEventListener("play", function () {
  videoContainer.className = videoContainer.className.replace(/\bpaused\b/g, "").trim();
  showControls();
  resetInactivityTimer();
});

video.addEventListener("pause", function () {
  clearInactivityTimer();
  showControls();

  if (videoContainer.className.indexOf("paused") === -1) {
    videoContainer.className += " paused";
  }
});

video.addEventListener("ended", function () {
  clearInactivityTimer();
  showControls();
});

video.addEventListener("error", function () {
  clearInactivityTimer();
  showControls();
});

/* ===========================
   LOADING ICON (play/pause button shows loading.gif while video loads; IE-safe className)
=========================== */

function addVideoEvent(el, eventName, fn) {
  if (!el) return;
  if (el.attachEvent) el.attachEvent("on" + eventName, fn);
  else el.addEventListener(eventName, fn);
}

function setLoading(show) {
  if (!videoContainer) return;
  var cn = videoContainer.className;
  if (show) {
    if (cn.indexOf("loading") === -1) videoContainer.className = (cn + " loading").replace(/\s+/g, " ").trim();
  } else {
    videoContainer.className = cn.replace(/\bloading\b/g, "").replace(/\s+/g, " ").trim();
  }
  if (playPauseBtn) {
    if (show) {
      playPauseBtn.setAttribute("disabled", "disabled");
    } else {
      playPauseBtn.removeAttribute("disabled");
    }
  }
}

function bindLoadingState() {
  function showLoading() { setLoading(true); }
  function hideLoading() { setLoading(false); }
  addVideoEvent(video, "loadstart", showLoading);
  addVideoEvent(video, "waiting", showLoading);
  addVideoEvent(video, "canplay", hideLoading);
  addVideoEvent(video, "canplaythrough", hideLoading);
  addVideoEvent(video, "playing", hideLoading);
  addVideoEvent(video, "error", hideLoading);
}
setLoading(true);
bindLoadingState();

/* ===========================
   SETTINGS PANEL (IE-safe: addEventListener + attachEvent, className)
=========================== */

function addClick(el, fn) {
  if (!el) return;
  if (el.attachEvent) el.attachEvent("onclick", fn);
  else el.addEventListener("click", fn);
}

function toggleSettingsPanel(e) {
  if (e && e.stopPropagation) e.stopPropagation();
  if (e && e.cancelBubble !== undefined) e.cancelBubble = true;
  if (hasClass(settingsPanel, "settings-panel-open")) {
    removeClass(settingsPanel, "settings-panel-open");
  } else {
    addClass(settingsPanel, "settings-panel-open");
  }
}

function closeSettingsPanelIfOutside(e) {
  var t = e.target || e.srcElement;
  if (!settingsPanel || !hasClass(settingsPanel, "settings-panel-open")) return;
  while (t && t !== document.body) {
    if (t === settingsPanel || t === settingsBtn) return;
    t = t.parentNode;
  }
  removeClass(settingsPanel, "settings-panel-open");
}

if (settingsBtn && settingsPanel) {
  addClick(settingsBtn, function (e) {
    toggleSettingsPanel(e);
  });
  if (document.attachEvent) {
    document.attachEvent("onclick", closeSettingsPanelIfOutside);
  } else {
    document.addEventListener("click", closeSettingsPanelIfOutside);
  }
}

if (settingsPanel) {
  addClick(settingsPanel, function (e) {
    if (e && e.stopPropagation) e.stopPropagation();
    if (e && e.cancelBubble !== undefined) e.cancelBubble = true;
  });
  var annotBtns = settingsPanel.querySelectorAll(".settings-annot-btn");
  for (var i = 0; i < annotBtns.length; i++) {
    (function (btn) {
      addClick(btn, function () {
        var parent = btn.parentNode;
        var siblings = parent.querySelectorAll(".settings-annot-btn");
        for (var j = 0; j < siblings.length; j++) {
          removeClass(siblings[j], "active");
        }
        addClass(btn, "active");
      });
    })(annotBtns[i]);
  }
  var speedSelect = settingsPanel.querySelector(".settings-speed");
  if (speedSelect) {
    if (speedSelect.attachEvent) {
      speedSelect.attachEvent("onchange", function () {
        var val = parseFloat(speedSelect.value, 10);
        if (!isNaN(val)) video.playbackRate = val;
      });
    } else {
      speedSelect.addEventListener("change", function () {
        var val = parseFloat(speedSelect.value, 10);
        if (!isNaN(val)) video.playbackRate = val;
      });
    }
  }
  var qualitySelect = settingsPanel.querySelector(".settings-quality");
  var codecSelect = settingsPanel.querySelector(".settings-codec");
  function buildSrcWithParams(baseSrc) {
    if (!baseSrc) return "";
    var src = baseSrc
      .replace(/\bquality=[^&]*&?/g, "")
      .replace(/\bcodec=[^&]*&?/g, "")
      .replace(/[&?]$/, "");
    var sep = src.indexOf("?") >= 0 ? "&" : "?";
    var quality = qualitySelect ? qualitySelect.value : "auto";
    var codec = codecSelect ? codecSelect.value : "";
    src = src + sep + "quality=" + quality;
    if (codec === "mpeg4") src = src + "&codec=mpeg4";
    return src;
  }
  function applySourceAndReload() {
    var base = video.src || video.getAttribute("src") || "";
    if (!base) return;
    var wasPlaying = !video.paused;
    var seekTo = video.currentTime;
    durationFromHeader = NaN;
    video.src = buildSrcWithParams(base);
    video.load();
    fetchDurationFromHeaders();
    function restorePlay() {
      video.removeEventListener("loadedmetadata", restorePlay);
      video.removeEventListener("canplay", restorePlay);
      if (seekTo > 0) video.currentTime = seekTo;
      if (wasPlaying) video.play();
    }
    video.addEventListener("loadedmetadata", restorePlay);
    video.addEventListener("canplay", restorePlay);
  }
  if (qualitySelect) {
    if (qualitySelect.attachEvent) {
      qualitySelect.attachEvent("onchange", applySourceAndReload);
    } else {
      qualitySelect.addEventListener("change", applySourceAndReload);
    }
  }
  if (codecSelect) {
    if (codecSelect.attachEvent) {
      codecSelect.attachEvent("onchange", applySourceAndReload);
    } else {
      codecSelect.addEventListener("change", applySourceAndReload);
    }
  }
}

/* ===========================
   MUTE / VOLUME
=========================== */

muteBtn.addEventListener("click", toggleMute);

var setSliderBackground = function (value) {
  if (!volumeSlider) return;
  var pct = Math.round(value * 100);
  volumeSlider.style.background =
    "linear-gradient(to right, rgba(255,255,255,0.9) " +
    pct +
    "%, rgba(255,255,255,0.35) " +
    pct +
    "%)";
};

if (volumeSlider) {
  var updateVolumeFromSlider = function (e) {
    var sliderValue = parseFloat(e.target.value);
    if (isNaN(sliderValue)) return;
    video.volume = sliderValue;
    video.muted = sliderValue === 0;
    setSliderBackground(sliderValue);
  };

  volumeSlider.addEventListener("input", updateVolumeFromSlider);
  volumeSlider.addEventListener("change", updateVolumeFromSlider);
  setSliderBackground(parseFloat(volumeSlider.value) || 1);
}

function toggleMute() {
  video.muted = !video.muted;
}

video.addEventListener("volumechange", function() {
  if (volumeSlider) {
    var sliderValue = video.muted ? 0 : video.volume;
    volumeSlider.value = sliderValue;
    setSliderBackground(sliderValue);
  }

  var volumeLevel;
  if (video.muted || video.volume === 0) {
    volumeLevel = "muted";
  } else if (video.volume >= 0.5) {
    volumeLevel = "high";
  } else {
    volumeLevel = "low";
  }
  videoContainer.setAttribute("data-volume-level", volumeLevel);
});

/* ===========================
   MINI PLAYER (PICTURE-IN-PICTURE)
=========================== */

miniPlayerBtn.addEventListener("click", toggleMiniPlayer);

function toggleMiniPlayer() {
  // Comprehensive feature detection for Picture-in-Picture
  var pipSupported = (
    document.pictureInPictureEnabled && 
    video.requestPictureInPicture &&
    typeof document.exitPictureInPicture === 'function'
  );
  
  // Additional check for older browsers including IE9
  var isModernBrowser = (
    window.Promise && 
    window.fetch && 
    typeof Object.assign === 'function'
  );
  
  // Detect IE specifically
  var isIE = (
    navigator.userAgent.indexOf('MSIE') !== -1 || 
    navigator.userAgent.indexOf('Trident') !== -1
  );
  
  if (!pipSupported || !isModernBrowser || isIE) {
    alert("The mini-player is not supported by your browser.");
    return;
  }

// Toggle Picture-in-Picture mode
  if (document.pictureInPictureElement) {
    // Exit PiP mode
    document.exitPictureInPicture()
      .catch(function(error) {
        console.error("Error when exiting the mini-player:", error);
      });
  } else {
    // Enter PiP mode
    video.requestPictureInPicture()
      .catch(function(error) {
        console.error("Error when logging into the mini-player:", error);
        alert("Couldn't activate the mini-player");
      });
  }
}

// Handle PiP events
video.addEventListener("enterpictureinpicture", function() {
  videoContainer.classList.add("mini-player-active");
});

video.addEventListener("leavepictureinpicture", function() {
  videoContainer.classList.remove("mini-player-active");
});

/* ===========================
   AUTO HIDE CONTROLS
=========================== */

var hideControlsTimer = null;
var hideControlsDelay = 3500;

function addClass(el, cls) {
  if (!el || !cls) return;
  var classes = el.className ? el.className.split(/\s+/) : [];
  for (var i = 0; i < classes.length; i++) {
    if (classes[i] === cls) return;
  }
  classes.push(cls);
  el.className = classes.join(" ").trim();
}

function removeClass(el, cls) {
  if (!el || !cls) return;
  el.className = el.className
    .split(/\s+/)
    .filter(function (value) {
      return value !== cls && value.length > 0;
    })
    .join(" ")
    .trim();
}

function hasClass(el, cls) {
  if (!el || !cls) return false;
  return (" " + (el.className || "") + " ").indexOf(" " + cls + " ") > -1;
}

function canTogglePlayback() {
  if (!video) return false;
  var hasSrc = !!(video.currentSrc || video.getAttribute("src"));
  if (!hasSrc) return false;
  if (video.error && video.error.code === 4) return false;
  return true;
}

function isTouchDevice() {
  if (typeof navigator === "undefined") return false;
  if ("maxTouchPoints" in navigator && navigator.maxTouchPoints > 0) return true;
  if ("msMaxTouchPoints" in navigator && navigator.msMaxTouchPoints > 0) return true;
  return /Mobi|Android|iPhone|iPad|iPod|Touch/.test(navigator.userAgent || "");
}

function clearInactivityTimer() {
  if (hideControlsTimer !== null) {
    clearTimeout(hideControlsTimer);
    hideControlsTimer = null;
  }
}

function resetInactivityTimer() {
  clearInactivityTimer();
  if (video.paused || isScrubbing) return;
  hideControlsTimer = setTimeout(hideControls, hideControlsDelay);
}

function hideControls() {
  if (video.paused || isScrubbing) return;
  addClass(videoContainer, "hide-controls");
}

function showControls() {
  removeClass(videoContainer, "hide-controls");
}

function handleUserActivity() {
  showControls();
  resetInactivityTimer();
}



var userActivityEvents = ["mousemove", "mousedown", "touchstart", "touchmove", "keydown", "wheel", "mousewheel", "pointermove", "pointerdown", "MSPointerMove", "MSPointerDown"];

userActivityEvents.forEach(function (eventName) {
  document.addEventListener(eventName, function (e) {
    if (eventName === "keydown") {
      handleUserActivity();
      return;
    }
    if (window.isEventInsidePlayer(e)) handleUserActivity();
  }, true);
});

var activityOverlay = document.querySelector(".video-activity-overlay");
if (videoContainer) {
  userActivityEvents.forEach(function (eventName) {
    videoContainer.addEventListener(eventName, handleUserActivity, true);
  });
}
if (activityOverlay) {
  userActivityEvents.forEach(function (eventName) {
    activityOverlay.addEventListener(eventName, handleUserActivity, true);
  });
  activityOverlay.addEventListener("click", function (e) {
    var wasHidden = hasClass(videoContainer, "hide-controls");
    handleUserActivity();
    if (wasHidden && video) {
      try {
        var ev = document.createEvent("MouseEvents");
        ev.initEvent("click", true, true);
        video.dispatchEvent(ev);
      } catch (err) {}
    }
  }, true);
}
