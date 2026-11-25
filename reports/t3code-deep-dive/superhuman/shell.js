/* Just Superhuman shell: theme, brand, app switcher, command palette, toasts.
   Shared by every app; edit in F:/JustSuperHuman/JustSuperHost/ui and run `npm run ui:sync`,
   which copies it to each project as superhuman/ (served at that relative path).

   <script src="superhuman/shell.js" defer></script> then use:
     <sh-brand app="JustPosts" href="./"></sh-brand>   mark + "Just Superhuman / JustPosts"
                                                         (inside the suite: the deployment's brand, if it has one)
     <sh-switcher></sh-switcher>                         app grid; hides itself outside the suite
   and from script:
     SH.command({ id, title, group, hint, keywords, run })  adds a ⌘K entry
     SH.toast('Saved') / SH.toast('Could not save', { error: true })
     SH.toast('Chat archived', { action: { label: 'Undo', run() {} } })
     SH.setTheme('light' | 'dark' | 'system')
   Add data-sh-palette="off" to <html> to leave ⌘K to the app. The switcher and
   suite entries appear only when the suite's sh-suite=1 cookie is present (or
   <html data-sh-session="/path"> names a session endpoint). */
(function () {
  'use strict';
  if (window.SH && window.SH.version) return;
  var doc = document, root = doc.documentElement;
  var THEME_KEY = 'sh-theme';
  var esc = function (s) { return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]; }); };
  var store = {
    get: function (k) { try { return localStorage.getItem(k); } catch (e) { return null; } },
    set: function (k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch (e) {} }
  };

  /* ---------- icons ---------- */
  var svg = function (body, cls) { return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"' + (cls ? ' class="' + cls + '"' : '') + '>' + body + '</svg>'; };
  var ICONS = {
    justconnect: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4M7 9h4m-2-2 2 2-2 2M14 12h3"/>',
    forms: '<rect x="5" y="4" width="14" height="17" rx="2"/><path d="M9 3h6v3H9zM9 10h6M9 14h6M9 18h3"/>',
    justimagine: '<path d="M12 3.5 13.7 8.3 18.5 10 13.7 11.7 12 16.5 10.3 11.7 5.5 10 10.3 8.3Z"/><path d="M18.5 15.5 19.3 17.7 21.5 18.5 19.3 19.3 18.5 21.5 17.7 19.3 15.5 18.5 17.7 17.7Z"/>',
    justmemes: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 8h10M8 16h8"/><path d="M9.5 12h.01M14.5 12h.01"/>',
    justreach: '<rect x="2" y="6" width="13" height="12" rx="2.4"/><path d="m2.8 8.1 5.7 4.4 5.7-4.4"/><path d="M18.5 9.2a4 4 0 0 1 0 5.6"/><path d="M20.9 6.6a7.6 7.6 0 0 1 0 10.8"/>',
    justposts: '<rect x="4" y="6" width="13" height="14" rx="2"/><path d="M8 3h10a2 2 0 0 1 2 2v11"/><path d="m9 10.5 4 2.5-4 2.5Z"/>',
    cast: '<circle cx="10" cy="8" r="3.6"/><path d="M3.5 20a6.5 6.5 0 0 1 13 0"/><path d="m18.5 3 .8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8Z"/>',
    sites: '<circle cx="12" cy="12" r="9"/><path d="M3.5 9h17M3.5 15h17"/><path d="M12 3c2.4 2.5 3.6 5.5 3.6 9s-1.2 6.5-3.6 9c-2.4-2.5-3.6-5.5-3.6-9S9.6 5.5 12 3Z"/>',
    studio: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M3 9h18"/><path d="m10 13-2 2 2 2M14 13l2 2-2 2"/>',
    assistant: '<path d="M4 5.5A2.5 2.5 0 0 1 6.5 3h11A2.5 2.5 0 0 1 20 5.5v8a2.5 2.5 0 0 1-2.5 2.5H10l-5 4.5V16.4A2.5 2.5 0 0 1 4 14.5Z"/><path d="m12 6.6.9 2.5 2.5.9-2.5.9-.9 2.5-.9-2.5-2.5-.9 2.5-.9Z"/>',
    analytics: '<path d="M4 20V11M10 20V5M16 20v-6M21 20H3"/>',
    home: '<path d="M3.5 10.5 12 4l8.5 6.5V19a1 1 0 0 1-1 1H15v-6H9v6H4.5a1 1 0 0 1-1-1Z"/>',
    users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0M16.5 4.6a3.5 3.5 0 0 1 0 6.8M21.5 20a6.5 6.5 0 0 0-3.6-5.8"/>',
    shield: '<path d="M12 3 4.5 6v5.5c0 4.6 3.2 8 7.5 9.5 4.3-1.5 7.5-4.9 7.5-9.5V6Z"/><path d="m9 12 2 2 4-4"/>',
    share: '<circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="m8.6 13.5 6.8 4M15.4 6.5l-6.8 4"/>',
    grid: '<circle cx="5" cy="5" r="1.6"/><circle cx="12" cy="5" r="1.6"/><circle cx="19" cy="5" r="1.6"/><circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/><circle cx="5" cy="19" r="1.6"/><circle cx="12" cy="19" r="1.6"/><circle cx="19" cy="19" r="1.6"/>',
    search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    moon: '<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5Z"/>',
    monitor: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>',
    logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>',
    arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
    menu: '<path d="M4 6h16M4 12h16M4 18h16"/>',
    close: '<path d="M18 6 6 18M6 6l12 12"/>',
    settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z"/>',
    command: '<path d="M9 6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3Z"/>'
  };
  var icon = function (name, cls) { return svg(ICONS[name] || ICONS.grid, cls); };
  var MARK = '<svg class="sh-mark" viewBox="0 0 64 64" aria-hidden="true"><path d="M13.12 40.27 6.26 40.27 23.89 9.73 37.43 33.18 40.94 33.18 23.89 3.65 1 43.31 11.36 43.31ZM36.68 15.67 40.11 9.73 57.74 40.27 30.66 40.27 28.91 43.31 63 43.31 40.11 3.65 34.92 12.63ZM46.2 48.37 49.63 54.31 14.37 54.31 27.91 30.86 26.15 27.82 9.11 57.35 54.89 57.35 49.71 48.37Z" fill="currentColor"/></svg>';
  // A deployment's custom apps carry their own icon paths and color (the suite checked them).
  var glyph = function (id, app) {
    var own = app && app.glyph && app.glyph.paths;
    return '<span class="sh-app-glyph" data-app="' + esc(id) + '"' + (own ? ' style="--sh-glyph:' + esc(app.glyph.color) + '"' : '') + '>' +
      (own ? svg(own.map(function (d) { return '<path d="' + esc(d) + '"/>'; }).join('')) : icon(ICONS[id] ? id : 'grid')) + '</span>';
  };

  /* ---------- theme ---------- */
  // A brand may set the theme people see until they pick one (Admin → Brand: "Theme").
  var brandMode = function () {
    var b = window.SH_BRAND || null;
    if (!b) try { b = JSON.parse(store.get('sh-brand') || 'null'); } catch (e) {}
    return b && (b.mode === 'light' || b.mode === 'dark') ? b.mode : '';
  };
  var applyTheme = function (mode) {
    if (mode !== 'light' && mode !== 'dark') mode = brandMode() || mode;
    if (mode === 'light' || mode === 'dark') root.setAttribute('data-theme', mode);
    else root.removeAttribute('data-theme');
    var meta = doc.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', getComputedStyle(root).getPropertyValue('--sh-bg').trim() || '#f7f8fa');
  };
  var getTheme = function () { var t = store.get(THEME_KEY); return t === 'light' || t === 'dark' ? t : 'system'; };
  var setTheme = function (mode) { store.set(THEME_KEY, mode === 'system' ? null : mode); applyTheme(mode); doc.dispatchEvent(new CustomEvent('sh:theme', { detail: { theme: mode } })); };
  var isDark = function () { var t = root.getAttribute('data-theme'); return t ? t === 'dark' : !!(window.matchMedia && matchMedia('(prefers-color-scheme: dark)').matches); };
  if (!root.hasAttribute('data-sh-theme-locked')) applyTheme(getTheme());
  window.addEventListener('storage', function (e) { if (e.key === THEME_KEY) applyTheme(getTheme()); });

  /* ---------- session (suite only) ---------- */
  var sessionPromise = null;
  var session = function () {
    if (!sessionPromise && !/(?:^|; )sh-suite=1/.test(doc.cookie) && !root.hasAttribute('data-sh-session')) sessionPromise = Promise.resolve(null);
    if (!sessionPromise) {
      var url = root.getAttribute('data-sh-session') || '/auth/session';
      sessionPromise = fetch(url, { credentials: 'same-origin', headers: { accept: 'application/json' } })
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (d) { return d && d.user ? d : null; })
        .catch(function () { return null; });
    }
    return sessionPromise;
  };
  var here = function (href) {
    var p = location.pathname;
    if (href === '/') return p === '/';
    var h = href.replace(/\/$/, '');
    return p === h || p.indexOf(h + '/') === 0;
  };
  var signOut = function () {
    return fetch('/auth/logout', { method: 'POST', credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) throw new Error('Unable to sign out. Try again.');
      location.assign('/login');
    }).catch(function (e) { toast(e.message, { error: true }); });
  };

  /* ---------- brand (white label) ---------- */
  // Inside the suite, /auth/session names the deployment's brand (name, logo, mark,
  // accent) when it has one. The last one seen is kept so the next page draws it
  // at once; until the first answer, <sh-brand> keeps its place without a logo.
  var BRAND_KEY = 'sh-brand';
  var inSuite = function () { return /(?:^|; )sh-suite=1/.test(doc.cookie) || root.hasAttribute('data-sh-session'); };
  var brand = null, brandKnown = !inSuite();
  if (inSuite()) try { var saved = JSON.parse(store.get(BRAND_KEY) || 'null'); if (saved && saved.name) { brand = saved; brandKnown = true; } } catch (e) {}
  var firstIcon = doc.querySelector('link[rel~="icon"]'), defaultIcon = firstIcon ? [firstIcon.getAttribute('href'), firstIcon.getAttribute('type')] : null;
  var brandHead = function (b) {
    var href = b && (b.mark || b.logo), links = doc.querySelectorAll('link[rel~="icon"]');
    if (href || defaultIcon) links.forEach(function (l) { l.setAttribute('href', href || defaultIcon[0]); var type = href ? b.markType : defaultIcon[1]; if (type) l.setAttribute('type', type); else l.removeAttribute('type'); });
    // The suite's own pages inline the accent (<style id="sh-brand-theme">); apps get it as a stylesheet.
    var theme = doc.getElementById('sh-brand-theme');
    if (b && b.theme && !theme) { theme = doc.createElement('link'); theme.id = 'sh-brand-theme'; theme.rel = 'stylesheet'; theme.href = b.theme; doc.head.appendChild(theme); }
    else if ((!b || !b.theme) && theme && theme.tagName === 'LINK') theme.remove();
  };
  // A brand whose mark is just its logo file has no square mark to show where space is tight.
  var ownMark = function (b) { return !!b.mark && !(b.logo && b.mark.split('?v=')[1] === b.logo.split('?v=')[1]); };
  var brandInner = function (b, compact) {
    var img = function (src, cls) { return '<img class="' + cls + '" src="' + esc(src) + '" alt="">'; };
    // Compact without a mark of its own: a smaller logo takes the mark's place (and the app name's room).
    var small = compact ? ' sh-brand-logo--compact' : '';
    // On a phone the wide logo would push the bar off the screen; the square mark (when there is one) takes its place.
    if (b.logo && (!compact || !ownMark(b))) return img(b.logo, 'sh-brand-logo sh-brand-logo--light' + small + (b.logoDark ? ' has-dark' : ' sh-brand-logo--chip')) + (b.logoDark ? img(b.logoDark, 'sh-brand-logo sh-brand-logo--dark' + small) : '') + (!compact && ownMark(b) ? img(b.mark, 'sh-brand-mark sh-brand-mark--narrow') : '');
    return (b.mark ? img(b.mark, 'sh-brand-mark') : '') + (compact ? '' : '<span class="sh-wordmark sh-wordmark--client">' + esc(b.name) + '</span>');
  };
  var redrawBrands = function () { doc.querySelectorAll('sh-brand').forEach(function (el) { if (el._draw) el._draw(); }); };
  // The brand's sample text (Admin → Brand): SH.text('meme', 'work') in scripts, and in markup
  // <input placeholder="like work" data-sh-placeholder="meme" data-sh-prefix="like "> (data-sh-first keeps
  // only the first of a comma list). Without a brand, or a value, the page's own text stays.
  var text = function (key, fallback) {
    var c = (brand && brand.content) || (window.SH_BRAND && window.SH_BRAND.content) || null, v = c ? c[key] : null;
    if (Array.isArray(v)) v = v.join(', ');
    return v == null || v === '' ? (fallback == null ? '' : fallback) : String(v);
  };
  var applyText = function () {
    doc.querySelectorAll('[data-sh-placeholder]').forEach(function (el) {
      if (!el.hasAttribute('data-sh-default')) el.setAttribute('data-sh-default', el.getAttribute('placeholder') || '');
      var v = text(el.getAttribute('data-sh-placeholder'), '');
      if (v && el.hasAttribute('data-sh-first')) v = v.split(',')[0].trim();
      el.setAttribute('placeholder', v ? (el.getAttribute('data-sh-prefix') || '') + v : el.getAttribute('data-sh-default'));
    });
  };
  var setBrand = function (b) {
    brand = b && b.name ? b : null; brandKnown = true;
    store.set(BRAND_KEY, brand ? JSON.stringify(brand) : null);
    brandHead(brand); redrawBrands(); applyText();
    if (getTheme() === 'system' && !root.hasAttribute('data-sh-theme-locked')) applyTheme('system');
  };
  if (brand) brandHead(brand);
  if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', applyText); else applyText();
  if (inSuite()) session().then(function (s) { if (s) setBrand(s.brand || null); else if (!brandKnown) { brandKnown = true; redrawBrands(); } });

  /* ---------- toast ---------- */
  var toastTimer = null, toastEl = null;
  var toast = function (message, opts) {
    opts = opts || {};
    if (toastEl) toastEl.remove();
    var el = toastEl = doc.createElement('div');
    toastEl.className = 'sh-toast' + (opts.error ? ' sh-toast--error' : '');
    toastEl.setAttribute('role', opts.error ? 'alert' : 'status');
    toastEl.textContent = message;
    if (opts.action) {
      var act = doc.createElement('button');
      act.type = 'button';
      act.className = 'sh-toast-action';
      act.textContent = opts.action.label;
      act.addEventListener('click', function () { el.remove(); if (toastEl === el) toastEl = null; opts.action.run(); });
      toastEl.appendChild(act);
    }
    doc.body.appendChild(toastEl);
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { el.remove(); if (toastEl === el) toastEl = null; }, opts.duration || (opts.error || opts.action ? 6000 : 3200));
  };

  /* ---------- <sh-brand> ---------- */
  if (!customElements.get('sh-brand')) customElements.define('sh-brand', class extends HTMLElement {
    connectedCallback() { this._draw(); }
    _draw() {
      var app = this.getAttribute('app'), href = this.getAttribute('href') || '/';
      var compact = this.hasAttribute('compact');
      var appName = app ? '<span class="sh-brand-app' + (compact ? ' sh-brand-app--compact' : '') + '">' + esc(app) + '</span>' : '';
      if (brand) { this.innerHTML = '<a class="sh-brand sh-brand--client" href="' + esc(href) + '" aria-label="' + esc(brand.name + (app ? ' ' + app : '') + ' home') + '">' + brandInner(brand, compact) + (compact && brand.logo && !ownMark(brand) ? '' : appName) + '</a>'; return; }
      if (!brandKnown) { this.innerHTML = '<a class="sh-brand sh-brand--pending" href="' + esc(href) + '" aria-label="' + esc((app ? app + ' ' : '') + 'home') + '">' + appName + '</a>'; return; }
      this.innerHTML = '<a class="sh-brand" href="' + esc(href) + '" aria-label="' + esc('Just Superhuman' + (app ? ' ' + app : '') + ' home') + '">' + MARK +
        (compact ? '' : '<span class="sh-wordmark">Just <b>Superhuman</b></span>') + appName + '</a>';
    }
  });

  /* ---------- <sh-switcher> ---------- */
  var openPopover = null;
  var closePopover = function () {
    if (!openPopover) return;
    if (openPopover._button) openPopover._button.setAttribute('aria-expanded', 'false');
    openPopover.remove(); openPopover = null;
  };
  window.addEventListener('resize', closePopover);
  doc.addEventListener('pointerdown', function (e) { if (openPopover && !openPopover.contains(e.target) && !e.target.closest('sh-switcher')) closePopover(); });
  doc.addEventListener('keydown', function (e) { if (e.key === 'Escape' && openPopover) { var b = openPopover._button; closePopover(); if (b) b.focus(); } });

  if (!customElements.get('sh-switcher')) customElements.define('sh-switcher', class extends HTMLElement {
    connectedCallback() {
      var self = this;
      if (this._ready) return; this._ready = true;
      this.hidden = true;
      session().then(function (s) {
        if (!s) return;
        self.hidden = false;
        self.innerHTML = '<button type="button" class="sh-btn sh-btn--ghost sh-btn--icon" aria-label="All apps" aria-haspopup="true" aria-expanded="false" title="All apps">' + icon('grid') + '</button>';
        var button = self.querySelector('button');
        button.addEventListener('click', function () {
          if (openPopover && openPopover._button === button) { closePopover(); button.setAttribute('aria-expanded', 'false'); return; }
          closePopover();
          var apps = s.apps || [];
          var pop = doc.createElement('div');
          pop.className = 'sh-popover sh-switcher-panel';
          pop.setAttribute('role', 'menu');
          pop.innerHTML = '<div class="sh-switcher-grid">' +
            apps.map(function (a) { return '<a class="sh-switcher-app" role="menuitem" href="' + esc(a.href) + '"' + (here(a.href) ? ' aria-current="page"' : '') + '>' + glyph(a.id, a) + '<span>' + esc(a.name) + '</span></a>'; }).join('') +
            '</div><div class="sh-switcher-foot"><a class="sh-btn sh-btn--ghost sh-btn--sm" href="/">' + icon('home') + 'Home</a>' +
            '<span style="display:flex;gap:4px"><button type="button" class="sh-btn sh-btn--ghost sh-btn--sm sh-btn--icon" data-act="theme" aria-label="Switch theme" title="Switch theme">' + icon(isDark() ? 'sun' : 'moon') + '</button>' +
            '<button type="button" class="sh-btn sh-btn--ghost sh-btn--sm sh-btn--icon" data-act="out" aria-label="Sign out" title="Sign out">' + icon('logout') + '</button></span></div>';
          // The top layer keeps the panel above every app's own stacking
          // (sidebars, sheets, sticky bars); older browsers fall back to z-index.
          pop.setAttribute('popover', 'manual');
          doc.body.appendChild(pop);
          if (pop.showPopover) try { pop.showPopover(); } catch (e) {}
          pop._button = button;
          var r = button.getBoundingClientRect(), w = pop.offsetWidth;
          pop.style.left = Math.min(Math.max(8, r.left), doc.documentElement.clientWidth - w - 8) + 'px';
          pop.style.top = (r.bottom + 6) + 'px';
          pop.querySelector('[data-act="theme"]').addEventListener('click', function () { setTheme(isDark() ? 'light' : 'dark'); this.innerHTML = icon(isDark() ? 'sun' : 'moon'); });
          pop.querySelector('[data-act="out"]').addEventListener('click', signOut);
          openPopover = pop;
          button.setAttribute('aria-expanded', 'true');
          var first = pop.querySelector('a'); if (first) first.focus();
        });
      });
    }
  });

  /* ---------- command palette ---------- */
  var commands = [];
  var command = function (c) { commands = commands.filter(function (x) { return x.id !== c.id; }); commands.push(c); return function () { commands = commands.filter(function (x) { return x !== c; }); }; };
  var builtins = function (s) {
    var list = [];
    if (s) {
      list.push({ id: 'sh:home', group: 'Go to', title: 'Home', icon: 'home', run: function () { location.assign('/'); } });
      (s.apps || []).forEach(function (a) { list.push({ id: 'sh:app:' + a.id, group: 'Apps', title: a.name, hint: a.description, glyph: a.id, app: a, keywords: a.id, run: function () { location.assign(a.href); } }); });
      if (s.user && s.user.isAdmin) {
        // The admin pages this person may open right now (the suite sends them; older suites send none).
        var shown = s.admin || ['accounts', 'roles', 'social', 'brand'];
        var admin = function (id, item) { if (shown.indexOf(id) !== -1) list.push(item); };
        admin('accounts', { id: 'sh:accounts', group: 'Admin', title: 'Accounts', icon: 'users', keywords: 'people users members invite', run: function () { location.assign('/admin'); } });
        admin('roles', { id: 'sh:roles', group: 'Admin', title: 'Roles and permissions', icon: 'shield', run: function () { location.assign('/admin/roles'); } });
        admin('social', { id: 'sh:social', group: 'Admin', title: 'Social accounts', icon: 'share', keywords: 'instagram facebook linkedin tiktok threads connect adaptlypost', run: function () { location.assign('/admin/social'); } });
        admin('settings', { id: 'sh:settings', group: 'Admin', title: 'Settings', icon: 'settings', keywords: 'api keys providers apify storage s3 email postmark adaptlypost', run: function () { location.assign('/admin/settings'); } });
        admin('github', { id: 'sh:github', group: 'Admin', title: 'GitHub', icon: 'settings', keywords: 'repositories deploy keys webhook', run: function () { location.assign('/admin/github'); } });
        admin('brand', { id: 'sh:brand', group: 'Admin', title: 'Brand guide', icon: 'sun', keywords: 'logo colors fonts design', run: function () { location.assign('/admin/brand'); } });
        list.push({ id: 'sh:status', group: 'Admin', title: 'System status', icon: 'monitor', keywords: 'cpu memory ram disk storage claude usage bro server health', run: function () { location.assign('/status'); } });
      }
    }
    list.push({ id: 'sh:theme-light', group: 'Appearance', title: 'Light theme', icon: 'sun', keywords: 'theme mode', run: function () { setTheme('light'); } });
    list.push({ id: 'sh:theme-dark', group: 'Appearance', title: 'Dark theme', icon: 'moon', keywords: 'theme mode', run: function () { setTheme('dark'); } });
    list.push({ id: 'sh:theme-system', group: 'Appearance', title: 'Match system theme', icon: 'monitor', keywords: 'theme mode auto', run: function () { setTheme('system'); } });
    if (s) list.push({ id: 'sh:out', group: 'Account', title: 'Sign out', icon: 'logout', run: signOut });
    return list;
  };
  var score = function (c, q) {
    if (!q) return 1;
    var hay = (c.title + ' ' + (c.keywords || '') + ' ' + (c.group || '')).toLowerCase();
    var title = c.title.toLowerCase();
    if (title.indexOf(q) === 0) return 4;
    if (title.indexOf(q) > -1) return 3;
    if (hay.indexOf(q) > -1) return 2;
    var i = 0; for (var j = 0; j < hay.length && i < q.length; j++) if (hay[j] === q[i]) i++;
    return i === q.length ? 1 : 0;
  };
  var palette = null;
  var openPalette = function () {
    if (palette && palette.open) return;
    session().then(function (s) {
      if (!palette) {
        palette = doc.createElement('dialog');
        palette.className = 'sh-palette';
        palette.setAttribute('aria-label', 'Command menu');
        palette.innerHTML = '<div class="sh-palette-input">' + icon('search') + '<input type="text" placeholder="Search apps and actions…" aria-label="Search apps and actions" autocomplete="off" spellcheck="false" role="combobox" aria-expanded="true" aria-controls="sh-palette-list"></div><div class="sh-palette-list" id="sh-palette-list" role="listbox"></div><div class="sh-palette-foot"><span><kbd class="sh-kbd">↑</kbd><kbd class="sh-kbd">↓</kbd> move</span><span><kbd class="sh-kbd">↵</kbd> open</span><span><kbd class="sh-kbd">esc</kbd> close</span></div>';
        doc.body.appendChild(palette);
        palette.addEventListener('click', function (e) { if (e.target === palette) palette.close(); });
        palette.addEventListener('close', function () { if (palette._return) palette._return.focus && palette._return.focus(); });
      }
      var input = palette.querySelector('input'), listEl = palette.querySelector('.sh-palette-list');
      var all = commands.concat(builtins(s)), shown = [], sel = 0;
      var render = function () {
        var q = input.value.trim().toLowerCase();
        shown = all.map(function (c) { return { c: c, s: score(c, q) }; }).filter(function (x) { return x.s > 0; })
          .sort(function (a, b) { return q ? b.s - a.s : 0; }).map(function (x) { return x.c; });
        if (sel >= shown.length) sel = Math.max(0, shown.length - 1);
        if (!shown.length) { listEl.innerHTML = '<div class="sh-palette-empty">Nothing matches “' + esc(input.value) + '”.</div>'; return; }
        var html = '', group = null;
        shown.forEach(function (c, i) {
          if (!q && c.group !== group) { group = c.group; html += '<div class="sh-palette-group">' + esc(group || 'Actions') + '</div>'; }
          html += '<button type="button" class="sh-palette-item" role="option" id="sh-opt-' + i + '" data-i="' + i + '" aria-selected="' + (i === sel) + '">' +
            (c.glyph ? glyph(c.glyph, c.app) : '<span class="sh-app-glyph" style="--sh-glyph:var(--sh-surface-3);color:var(--sh-ink-2)">' + icon(c.icon || 'arrow') + '</span>') +
            '<span>' + esc(c.title) + '</span>' + (q && c.group ? '<small>' + esc(c.group) + '</small>' : '') + '</button>';
        });
        listEl.innerHTML = html;
        input.setAttribute('aria-activedescendant', 'sh-opt-' + sel);
      };
      var move = function (d) { if (!shown.length) return; sel = (sel + d + shown.length) % shown.length; render(); var el = listEl.querySelector('[aria-selected="true"]'); if (el) el.scrollIntoView({ block: 'nearest' }); };
      var run = function (i) { var c = shown[i]; if (!c) return; palette.close(); try { c.run(); } catch (e) { toast(e.message || String(e), { error: true }); } };
      input.oninput = function () { sel = 0; render(); };
      input.onkeydown = function (e) {
        if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
        else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
        else if (e.key === 'Enter') { e.preventDefault(); run(sel); }
      };
      listEl.onclick = function (e) { var b = e.target.closest('[data-i]'); if (b) run(+b.getAttribute('data-i')); };
      listEl.onmousemove = function (e) { var b = e.target.closest('[data-i]'); if (b && +b.getAttribute('data-i') !== sel) { sel = +b.getAttribute('data-i'); listEl.querySelectorAll('[aria-selected="true"]').forEach(function (x) { x.setAttribute('aria-selected', 'false'); }); b.setAttribute('aria-selected', 'true'); } };
      input.value = ''; sel = 0; render();
      palette._return = doc.activeElement;
      palette.showModal();
      input.focus();
    });
  };
  doc.addEventListener('keydown', function (e) {
    if (root.getAttribute('data-sh-palette') === 'off') return;
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && (e.key === 'k' || e.key === 'K')) { e.preventDefault(); openPalette(); }
  });
  doc.addEventListener('click', function (e) {
    var t = e.target.closest && e.target.closest('[data-sh-open-palette]'); if (t) { e.preventDefault(); openPalette(); }
    var m = e.target.closest && e.target.closest('[data-sh-nav-toggle]');
    if (m) { var app = m.closest('.sh-app') || doc.querySelector('.sh-app'); if (app) { app.toggleAttribute('data-nav-open'); m.setAttribute('aria-expanded', String(app.hasAttribute('data-nav-open'))); } }
    if (e.target.classList && e.target.classList.contains('sh-scrim')) { var a = e.target.closest('.sh-app'); if (a) a.removeAttribute('data-nav-open'); }
  });

  /* ---------- account credits, shared across every app and teammate ---------- */
  var creditBadge, creditDetails, creditState, creditEnabled = false;
  var drawCredits = function () {
    if (!creditBadge || !creditState) return;
    var b = creditState;
    creditBadge.textContent = b.available.toLocaleString() + ' credits';
    // Accounts from an invite code start on one-time test credits (`granted`, never expire) with no monthly allowance.
    var monthly = b.monthlyAllowance || 0, granted = b.granted || 0;
    creditBadge.title = monthly ? 'Shared account balance. Renews ' + new Date(b.resetsAt).toLocaleDateString() : 'Shared account balance. Test credits don’t expire.';
    if (creditDetails && creditDetails.open) {
      creditDetails.querySelector('[data-credit-details]').innerHTML = '<p><b>' + b.available.toLocaleString() + ' credits available</b></p>'
        + (monthly ? '<p>' + monthly.toLocaleString() + ' credits each month, shared across your team and apps.</p>' : '<p>Shared across your team and apps: images, video, voice and Claude all spend from this balance.</p>')
        + (granted ? '<p>' + granted.toLocaleString() + ' are test credits that never expire' + (monthly ? ', used after this month’s credits.' : '.') + '</p>' : '')
        + '<p>' + (monthly ? b.used.toLocaleString() + ' used · ' : '') + b.reserved.toLocaleString() + ' reserved for running requests.</p>'
        + (monthly ? '<p>Renews ' + esc(new Date(b.resetsAt).toLocaleString()) + '. Unused monthly credits expire.</p>' : '');
    }
  };
  var refreshCredits = function () {
    if (!creditBadge || doc.hidden && creditState) return Promise.resolve();
    return fetch('/api/credits', { credentials: 'same-origin', cache: 'no-store' }).then(function (r) { if (!r.ok) throw new Error(); return r.json(); }).then(function (d) { creditState = d.billing; drawCredits(); }).catch(function () { creditBadge.textContent = 'Credits unavailable'; });
  };
  var mountCredits = function () {
    if (!inSuite()) return;
    session().then(function (s) {
      if (!s || !s.creditMode) return;
      creditEnabled = true;
      creditBadge = doc.createElement('button');
      creditBadge.type = 'button'; creditBadge.className = 'sh-credit-badge'; creditBadge.textContent = 'Loading credits…';
      // JustImagine names its sidebar by id. Missing it puts the badge inside
      // its fixed-width brand row, pushing the app name into the gallery.
      var sidebar = doc.querySelector('.sh-sidebar, .sidebar, aside#sidebar');
      var brand = doc.querySelector('.jp-brand, .jm-brand') || doc.querySelector('sh-brand') || doc.querySelector('sh-switcher');
      if (sidebar) sidebar.appendChild(creditBadge); else if (brand) brand.insertAdjacentElement('afterend', creditBadge); else return;
      creditBadge.onclick = function () {
        if (!creditDetails) {
          creditDetails = doc.createElement('dialog'); creditDetails.className = 'sh-credit-dialog';
          creditDetails.setAttribute('aria-label', 'Account credits');
          creditDetails.innerHTML = '<h2>Account credits</h2><div data-credit-details></div><form method="dialog"><button class="sh-btn">Close</button></form>';
          doc.body.appendChild(creditDetails);
        }
        creditDetails.showModal(); drawCredits(); refreshCredits();
      };
      refreshCredits();
      setInterval(refreshCredits, 15000);
      doc.addEventListener('visibilitychange', refreshCredits);
      window.addEventListener('focus', refreshCredits);
    });
  };
  if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', mountCredits); else mountCredits();

  /* ---------- shortcut hints: ⌘K on Apple devices, Ctrl K elsewhere ---------- */
  var isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  var fixHints = function () { if (isMac) return; doc.querySelectorAll('kbd').forEach(function (k) { if (k.textContent === '⌘K') k.textContent = 'Ctrl K'; }); };
  if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', fixHints); else fixHints();

  var formatCost = function (usd) {
    if (creditEnabled) {
      if (!creditState) return 'Credit quote unavailable';
      var count = Math.max(0, Math.ceil(usd * creditState.creditsPerUsd - 1e-9));
      return count.toLocaleString() + (count === 1 ? ' credit' : ' credits');
    }
    return '$' + Number(usd).toFixed(usd < 1 ? 3 : 2);
  };
  window.SH = { version: 1, formatCost: formatCost, icon: icon, glyph: glyph, mark: MARK, brand: function () { return brand; }, text: text, toast: toast, command: command, openPalette: openPalette, setTheme: setTheme, getTheme: getTheme, isDark: isDark, session: session, signOut: signOut, escape: esc };
})();
