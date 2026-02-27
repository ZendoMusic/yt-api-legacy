var playPauseBtn = document.querySelector(".play-pause-btn");
var theaterBtn = document.querySelector(".theater-btn");
var fullScreenBtn = document.querySelector(".full-screen-btn");
var miniPlayerBtn = document.querySelector(".mini-player-btn");
var muteBtn = document.querySelector(".mute-btn");
var captionsBtn = document.querySelector(".captions-btn");
var speedBtn = document.querySelector(".speed-btn");
var currentTimeElem = document.querySelector(".current-time");
var totalTimeElem = document.querySelector(".total-time");
var volumeSlider = document.querySelector(".volume-slider");
var videoContainer = document.querySelector(".video-container");
var timelineContainer = document.querySelector(".timeline-container");
var video = document.querySelector("video");


/* ===========================
   INITIAL TIME
=========================== */

currentTimeElem.innerHTML = "0:00";
totalTimeElem.innerHTML = "0:00";

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

  html.className = html.className.replace(/\bie-fullscreen\b/g, "").trim();
  body.className = body.className.replace(/\bie-fullscreen\b/g, "").trim();
  videoContainer.className = videoContainer.className.replace(/\bfull-screen\b/g, "").trim();

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
    // ❗ выход ТОЛЬКО через ESC
    simulateEscapeKey();
    exitNativeFullscreen();
    exitIEFullscreen();
  } else {
    // вход БЕЗ ESC
    try {
      requestFullscreen(videoContainer);
    } catch (e) {
      enterIEFullscreen();
    }
  }
}

fullScreenBtn.addEventListener("click", toggleFullScreenMode);

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
    videoContainer.className = videoContainer.className.replace(/\bfull-screen\b/g, "").trim();
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

document.addEventListener("keydown", function (e) {
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
  if (e.key === "ArrowRight" || e.keyCode === 39) {
    if (video.currentTime <= video.duration - 10) {
      video.currentTime += 10;
    } else {
      video.currentTime = video.duration;
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
  if (!video.duration || isNaN(video.duration)) return;

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
  video.currentTime = getPercent(e) * video.duration;

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
  totalTimeElem.innerHTML = formatDuration(video.duration);
});

video.addEventListener("timeupdate", function () {
  if (!video.duration || isNaN(video.duration)) return;

  currentTimeElem.innerHTML = formatDuration(video.currentTime);

  if (isScrubbing) return;

  var percent = video.currentTime / video.duration;
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
   SPEED
=========================== */

speedBtn.addEventListener("click", function () {
  var rate = video.playbackRate + 0.25;
  if (rate > 2) rate = 0.25;
  video.playbackRate = rate;
  speedBtn.innerHTML = rate + "x";
});

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

["mousemove", "mousedown", "touchstart", "touchmove", "keydown", "wheel", "mousewheel", "pointermove", "pointerdown", "MSPointerMove", "MSPointerDown"].forEach(function (eventName) {
  document.addEventListener(eventName, handleUserActivity, true);
});
