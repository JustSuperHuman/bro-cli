window.__ModuleLoader__.load({
  id: '@bro-cli/dsh-profiles',
  factory: (require) => {
    var module = { exports: {} };
    var exports = module.exports;
    Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });

    const React = require('react');
    const h = React.createElement;
    const EMPTY_DIRECTORY = {
      current: null,
      routable: null,
      groups: [],
      failures: [],
      status: 'idle',
      error: null
    };
    const CSS = `
      .broProfilesLayer{flex:none;align-items:center;width:100%;height:49px;margin:8px 0 0;display:flex;position:relative}
      .broProfilesLayer.rail{width:36px;height:36px;margin:0}
      .broProfilesTrigger{width:100%;height:49px;color:var(--dsw-alias-label-primary);cursor:pointer;background:transparent;border:0;border-radius:12px;display:inline-flex;align-items:center;gap:8px;padding:0 8px 0 6px;font:inherit;font-size:14px;overflow:hidden}
      .broProfilesTrigger:hover,.broProfilesTrigger[aria-expanded=true]{background:var(--dsw-alias-interactive-bg-hover-solid)}
      .broProfilesLayer.rail .broProfilesTrigger{width:36px;height:36px;border-radius:50%;justify-content:center;padding:0}
      .broProfilesIcon{width:18px;height:18px;flex:none}
      .broProfilesTriggerLabel{min-width:0;text-overflow:ellipsis;white-space:nowrap;overflow:hidden}
      .broProfilesTriggerCount{color:var(--dsw-alias-label-tertiary);font-variant-numeric:tabular-nums;flex:none;margin-left:auto;font-size:12px}
      .broProfilesPanel{z-index:40;border:1px solid var(--dsw-alias-border-l1);background:var(--dsw-alias-bg-base);width:440px;max-width:calc(100vw - 24px);max-height:min(680px,calc(100vh - 160px));box-shadow:var(--dsw-shadow-lv2);border-radius:12px;display:flex;flex-direction:column;position:fixed;bottom:128px;left:12px;overflow:hidden;color:var(--dsw-alias-label-primary)}
      .broProfilesHeader{box-sizing:border-box;border-bottom:1px solid var(--dsw-alias-border-l2);display:flex;align-items:center;gap:8px;min-height:48px;padding:10px 12px}
      .broProfilesTitle{font-size:13px;font-weight:500;line-height:20px}
      .broProfilesUpdated{color:var(--dsw-alias-label-tertiary);font-size:11px;margin-left:auto;white-space:nowrap}
      .broProfilesIconButton{width:28px;height:28px;border:0;border-radius:999px;background:transparent;color:var(--dsw-alias-label-tertiary);cursor:pointer;font:inherit;font-size:17px;line-height:28px;padding:0}
      .broProfilesIconButton:hover:not(:disabled){background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-secondary)}
      .broProfilesIconButton:disabled{opacity:.4;cursor:default}
      .broProfilesBody{flex:1;min-height:0;padding:4px 12px 12px;overflow-y:auto}
      .broProfilesNote,.broProfilesError{color:var(--dsw-alias-label-tertiary);margin:8px 0;font-size:12px;line-height:18px}
      .broProfilesError{color:var(--dsw-alias-state-error-primary)}
      .broProfilesGroup{color:var(--dsw-alias-label-caption);text-transform:uppercase;letter-spacing:.04em;margin:10px 0 5px;font-size:11px;font-weight:500;line-height:16px}
      .broProfilesRows{list-style:none;display:flex;flex-direction:column;gap:6px;margin:0;padding:0}
      .broProfilesRow{width:100%;text-align:left;border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-base);color:inherit;border-radius:10px;display:flex;flex-direction:column;gap:7px;padding:9px 11px;font:inherit;cursor:pointer}
      .broProfilesRow:hover:not(:disabled){background:var(--dsw-alias-interactive-bg-hover)}
      .broProfilesRow[data-current=true]{border-color:var(--dsw-alias-border-l1)}
      .broProfilesRow:disabled{cursor:default;opacity:.55}
      .broProfilesRowHead{display:flex;align-items:baseline;gap:7px;min-width:0}
      .broProfilesName{min-width:0;text-overflow:ellipsis;white-space:nowrap;overflow:hidden;font-size:13px;font-weight:500;line-height:18px}
      .broProfilesPlan{color:var(--dsw-alias-label-tertiary);font-size:11px;line-height:16px;text-transform:capitalize}
      .broProfilesCurrent{color:var(--dsw-alias-state-success-primary);font-size:11px;line-height:16px;margin-left:auto;white-space:nowrap}
      .broProfilesState{color:var(--dsw-alias-label-tertiary);font-size:11px;line-height:16px;margin-left:auto;white-space:nowrap}
      .broProfilesMeters{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:8px}
      .broProfilesMeterHead{display:flex;align-items:center;justify-content:space-between;gap:4px;color:var(--dsw-alias-label-tertiary);font-size:10px;line-height:14px;font-variant-numeric:tabular-nums}
      .broProfilesTrack{height:3px;border-radius:2px;background:var(--dsw-alias-button-ghost-active-fill);overflow:hidden;margin-top:3px}
      .broProfilesFill{height:100%;border-radius:inherit;background:var(--dsw-alias-state-success-primary)}
      .broProfilesFill[data-pressure=mid]{background:var(--dsw-alias-state-warn-primary)}
      .broProfilesFill[data-pressure=high]{background:var(--dsw-alias-state-error-primary)}
      .broProfilesModel{color:var(--dsw-alias-label-caption);font-family:var(--dsh-font-mono,monospace);font-size:10px;line-height:14px;text-overflow:ellipsis;white-space:nowrap;overflow:hidden}
      @media(max-width:560px){.broProfilesPanel{left:8px;bottom:104px;max-width:calc(100vw - 16px)}.broProfilesMeters{gap:5px}}
    `;

    function installStyles() {
      if (document.getElementById('bro-dsh-profiles-css')) return;
      const style = document.createElement('style');
      style.id = 'bro-dsh-profiles-css';
      style.textContent = CSS;
      document.head.append(style);
    }

    function ProfilesIcon() {
      return h('svg', {
        className: 'broProfilesIcon', viewBox: '0 0 20 20', fill: 'none',
        stroke: 'currentColor', strokeWidth: 1.5, 'aria-hidden': true
      },
      h('circle', { cx: 7, cy: 7, r: 2.5 }),
      h('path', { d: 'M2.8 15.2c.5-2.5 2-3.8 4.2-3.8s3.7 1.3 4.2 3.8' }),
      h('circle', { cx: 14, cy: 6.2, r: 2 }),
      h('path', { d: 'M12.6 10.6c2.6-.6 4.2.7 4.6 3' }));
    }

    const clampPercent = (value) => Math.max(0, Math.min(100, Number(value) || 0));
    const percentText = (value) => Number.isFinite(Number(value)) ? `${Math.round(Number(value))}%` : '—';

    function Meter({ label, value }) {
      const percent = clampPercent(value);
      const pressure = percent >= 80 ? 'high' : percent >= 50 ? 'mid' : 'low';
      return h('div', { className: 'broProfilesMeter' },
        h('div', { className: 'broProfilesMeterHead' },
          h('span', null, label),
          h('span', null, percentText(value))),
        h('div', { className: 'broProfilesTrack' },
          h('div', {
            className: 'broProfilesFill',
            'data-pressure': pressure,
            style: { width: `${percent}%` }
          })));
    }

    function windowLabel(window, fallback) {
      const minutes = Number(window?.windowDurationMins);
      if (!Number.isFinite(minutes) || minutes <= 0) return fallback;
      if (minutes === 300) return '5h';
      if (minutes === 10_080) return 'week';
      if (minutes % 1_440 === 0) return `${minutes / 1_440}d`;
      if (minutes % 60 === 0) return `${minutes / 60}h`;
      return `${minutes}m`;
    }

    function usageMeters(profile) {
      const usage = profile.usage;
      if (!usage) return null;
      if (profile.kind === 'claude') {
        return h('div', { className: 'broProfilesMeters' },
          h(Meter, { label: '5h', value: usage.session }),
          h(Meter, { label: 'week', value: usage.weekly }),
          h(Meter, { label: 'Fable', value: usage.fable }));
      }
      const windows = [usage.primary, usage.secondary].filter(Boolean);
      if (!windows.length) return null;
      return h('div', {
        className: 'broProfilesMeters',
        style: { gridTemplateColumns: `repeat(${windows.length},minmax(0,1fr))` }
      }, windows.map((window, index) => h(Meter, {
        key: index,
        label: windowLabel(window, index === 0 ? 'primary' : 'secondary'),
        value: window.usedPercent
      })));
    }

    function stateText(profile, hasSession, addressed) {
      if (!profile.authenticated) return 'Logged out';
      if (profile.routeError || !profile.available) return 'Route unavailable';
      if (!hasSession) return 'No session';
      if (addressed) return 'Main session only';
      if (profile.usageError) return 'Usage unavailable';
      return '';
    }

    const emptySubscribe = () => () => {};
    const emptySnapshot = () => EMPTY_DIRECTORY;

    function ProfilePanel({ wide, useSessions, sessions, modelDirectories }) {
      const currentSession = useSessions((state) => state.current);
      const [open, setOpen] = React.useState(false);
      const [catalog, setCatalog] = React.useState({ profiles: [], refreshedAt: '' });
      const [loading, setLoading] = React.useState(false);
      const [error, setError] = React.useState('');
      const [selecting, setSelecting] = React.useState('');
      const addressed = currentSession !== undefined && currentSession !== null
        ? sessions.subagentAddress(currentSession) !== undefined
        : false;
      const directory = React.useMemo(() => {
        if (currentSession === undefined || currentSession === null) return null;
        try {
          return modelDirectories.directoryFor(currentSession);
        } catch {
          return null;
        }
      }, [currentSession, modelDirectories]);
      const directorySnapshot = React.useSyncExternalStore(
        directory ? (listener) => directory.store.subscribe(listener) : emptySubscribe,
        directory ? () => directory.store.getSnapshot() : emptySnapshot,
        emptySnapshot
      );

      const refresh = React.useCallback(async (force = false) => {
        setLoading(true);
        setError('');
        try {
          const response = await fetch(`/bro-profiles${force ? '?refresh=1' : ''}`, {
            cache: 'no-store',
            headers: { accept: 'application/json' }
          });
          if (!response.ok) throw new Error(`Profile list failed (${response.status})`);
          const body = await response.json();
          setCatalog({
            profiles: Array.isArray(body.profiles) ? body.profiles : [],
            refreshedAt: typeof body.refreshedAt === 'string' ? body.refreshedAt : ''
          });
        } catch (reason) {
          setError(reason instanceof Error ? reason.message : 'Profile list unavailable');
        } finally {
          setLoading(false);
        }
      }, []);

      React.useEffect(() => {
        if (!open) return;
        refresh(false);
        directory?.load().catch(() => {});
      }, [open, refresh, directory]);

      const selectProfile = async (profile) => {
        if (!directory || addressed || !profile.available || selecting) return;
        setSelecting(profile.id);
        setError('');
        try {
          const currentModel = directory.store.getSnapshot().current?.model;
          const model = profile.models.includes(currentModel) ? currentModel : profile.defaultModel;
          if (!profile.route || !model) throw new Error('This profile has no selectable model route');
          await directory.select({ provider: profile.route, model });
          setOpen(false);
        } catch (reason) {
          setError(reason instanceof Error ? reason.message : 'Could not switch profile');
        } finally {
          setSelecting('');
        }
      };

      const activeRoute = directorySnapshot.current?.provider || '';
      const groups = [
        ['claude', 'Claude profiles'],
        ['codex', 'Codex profiles']
      ];
      const ready = catalog.profiles.filter((profile) => profile.available).length;
      const refreshed = catalog.refreshedAt
        ? new Date(catalog.refreshedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
        : '';

      const renderRow = (profile) => {
        const active = Boolean(profile.route && profile.route === activeRoute);
        const state = stateText(profile, Boolean(directory), addressed);
        const disabled = !profile.available || !directory || addressed || Boolean(selecting);
        return h('li', { key: profile.id },
          h('button', {
            type: 'button',
            className: 'broProfilesRow',
            'data-current': active || undefined,
            disabled,
            onClick: () => selectProfile(profile),
            title: disabled ? state : `Use ${profile.label}`
          },
          h('div', { className: 'broProfilesRowHead' },
            h('span', { className: 'broProfilesName' }, profile.label),
            profile.plan && h('span', { className: 'broProfilesPlan' }, `· ${profile.plan}`),
            active
              ? h('span', { className: 'broProfilesCurrent' }, '✓ Current')
              : state && h('span', { className: 'broProfilesState' }, state),
            selecting === profile.id && h('span', { className: 'broProfilesState' }, 'Switching…')),
          usageMeters(profile),
          profile.available && h('div', { className: 'broProfilesModel' }, profile.defaultModel)));
      };

      return h('div', { className: `broProfilesLayer${wide ? '' : ' rail'}` },
        open && h('section', {
          className: 'broProfilesPanel',
          'aria-label': 'Claude and Codex profiles'
        },
        h('header', { className: 'broProfilesHeader' },
          h('span', { className: 'broProfilesTitle' }, 'Profiles'),
          refreshed && h('span', { className: 'broProfilesUpdated' }, `Updated ${refreshed}`),
          h('button', {
            type: 'button', className: 'broProfilesIconButton', title: 'Refresh usage',
            'aria-label': 'Refresh profile usage', disabled: loading, onClick: () => refresh(true)
          }, '↻'),
          h('button', {
            type: 'button', className: 'broProfilesIconButton', title: 'Close',
            'aria-label': 'Close profiles', onClick: () => setOpen(false)
          }, '×')),
        h('div', { className: 'broProfilesBody' },
          !directory && h('p', { className: 'broProfilesNote' }, 'Open a session to choose its login profile.'),
          addressed && h('p', { className: 'broProfilesNote' }, 'Profile switching is available from the main session.'),
          loading && catalog.profiles.length === 0 && h('p', { className: 'broProfilesNote' }, 'Loading profile usage…'),
          error && h('p', { className: 'broProfilesError', role: 'alert' }, error),
          !loading && !error && catalog.profiles.length === 0 && h('p', { className: 'broProfilesNote' }, 'No Claude or Codex profiles were found.'),
          groups.map(([kind, label]) => {
            const rows = catalog.profiles.filter((profile) => profile.kind === kind);
            if (!rows.length) return null;
            return h('section', { key: kind },
              h('h3', { className: 'broProfilesGroup' }, label),
              h('ul', { className: 'broProfilesRows' }, rows.map(renderRow)));
          }))),
        h('button', {
          type: 'button',
          className: 'broProfilesTrigger',
          'aria-label': 'Claude and Codex profiles',
          'aria-expanded': open,
          title: wide ? undefined : 'Profiles',
          onClick: () => setOpen((value) => !value)
        },
        h(ProfilesIcon),
        wide && h('span', { className: 'broProfilesTriggerLabel' }, 'Profiles'),
        wide && h('span', { className: 'broProfilesTriggerCount' }, ready || catalog.profiles.length || '')));
    }

    const inject = ['slots', 'sessions', 'modelDirectories'];
    function apply(ctx) {
      installStyles();
      ctx.slots.inject('sidebar.footer.action', () => ctx.slots.register({
        name: 'sidebar.footer.action',
        id: 'bro-profiles',
        order: -100,
        label: 'Profiles',
        inject: () => ({
          sessions: ctx.sessions,
          modelDirectories: ctx.modelDirectories
        })
      }, ProfilePanel));
    }

    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});
