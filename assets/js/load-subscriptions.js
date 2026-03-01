/**
 * Load subscriptions into #subscriptions-sidebar-content via /api/subscriptions_session.
 * ES5 / IE7-compatible: no arrow functions, no const/let, no fetch, no template literals.
 */
(function () {
  function escapeHtml(s) {
    if (s == null) return '';
    var str = String(s);
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function loadSubscriptions() {
    var container = document.getElementById('subscriptions-sidebar-content');
    if (!container) return;

    var xhr = new XMLHttpRequest();
    xhr.open('GET', '/api/subscriptions_session', true);
    xhr.withCredentials = true;

    xhr.onreadystatechange = function () {
      if (xhr.readyState !== 4) return;
      var html = '';
      try {
        if (xhr.status === 200 && xhr.responseText) {
          var data;
          try {
            data = JSON.parse(xhr.responseText);
          } catch (parseErr) {
            try {
              data = eval('(' + xhr.responseText + ')');
            } catch (e2) {
              data = { subscriptions: [], main_url: '' };
            }
          }
          var mainUrl = (data.main_url != null) ? String(data.main_url).replace(/\/+$/, '') : '';
          var subs = data.subscriptions;
          if (subs && subs.length > 0) {
            var i, sub, channelUrl, iconSrc, titleEsc, channelIdEsc, handleEnc;
            html = '<ul class="branded-page-related-channels-list">';
            for (i = 0; i < subs.length; i++) {
              sub = subs[i];
              handleEnc = encodeURIComponent(sub.title || '');
              channelUrl = mainUrl + '/channel?handle=' + handleEnc;
              iconSrc = sub.local_thumbnail && sub.local_thumbnail.length > 0
                ? sub.local_thumbnail
                : (sub.thumbnail && sub.thumbnail.length > 0 ? sub.thumbnail : '/assets/images/pixel-vfl3z5WfW.gif');
              titleEsc = escapeHtml(sub.title || '');
              channelIdEsc = escapeHtml(sub.channel_id || '');
              html += '<li class="branded-page-related-channels-item spf-link clearfix" data-external-id="' + channelIdEsc + '">';
              html += '<span class="yt-lockup clearfix yt-lockup-channel yt-lockup-mini">';
              html += '<div class="yt-lockup-thumbnail" style="width: 34px;">';
              html += '<a href="' + escapeHtml(channelUrl) + '" class="ux-thumb-wrap yt-uix-sessionlink spf-link">';
              html += '<span class="video-thumb yt-thumb yt-thumb-34 g-hovercard">';
              html += '<span class="yt-thumb-square"><span class="yt-thumb-clip">';
              html += '<img src="' + escapeHtml(iconSrc) + '" alt="Thumbnail" width="34" height="34">';
              html += '<span class="vertical-align"></span></span></span></span></a></div>';
              html += '<div class="yt-lockup-content">';
              html += '<span class="qualified-channel-title ellipsized"><span class="qualified-channel-title-wrapper">';
              html += '<span dir="ltr" class="qualified-channel-title-text g-hovercard">';
              html += '<h3 class="yt-lockup-title"><a class="yt-uix-sessionlink yt-uix-tile-link spf-link" dir="ltr" title="' + titleEsc + '" href="' + escapeHtml(channelUrl) + '">' + titleEsc + '</a></h3>';
              html += '</span></span></span></div></span></li>';
            }
            html += '</ul>';
          } else {
            html = '<p class="subscriptions-loading">No subscriptions</p>';
          }
        } else {
          html = '<p class="subscriptions-loading">No subscriptions</p>';
        }
      } catch (err) {
        html = '<p class="subscriptions-loading">No subscriptions</p>';
      }
      container.innerHTML = html;
    };

    xhr.send();
  }

  if (typeof window.addEventListener === 'function') {
    window.addEventListener('load', loadSubscriptions);
  } else if (typeof window.attachEvent === 'function') {
    window.attachEvent('onload', loadSubscriptions);
  } else {
    window.onload = loadSubscriptions;
  }
})();
