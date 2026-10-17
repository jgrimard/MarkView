// Shared by the app shell and the preview pages. Maps shortcuts to app commands and hands them to
// window.mvSendCommand (the shell runs them; a preview forwards them to the shell).
(() => {
    'use strict';
    const commands = {
        'ctrl+o': 'open', 'ctrl+n': 'new', 'ctrl+s': 'save', 'ctrl+shift+s': 'saveAs',
        'ctrl+w': 'closeDoc', 'ctrl+f4': 'closeDoc',
        'ctrl+tab': 'nextTab', 'ctrl+shift+tab': 'prevTab', 'ctrl+pagedown': 'nextTab', 'ctrl+pageup': 'prevTab',
        'ctrl+t': 'toggleToc', 'ctrl+e': 'cycleMode',
        'ctrl+1': 'modeView', 'ctrl+2': 'modeSplit', 'ctrl+3': 'modeEdit',
        'ctrl+p': 'print',
        'ctrl+=': 'zoomIn', 'ctrl++': 'zoomIn', 'ctrl+shift++': 'zoomIn', 'ctrl+-': 'zoomOut', 'ctrl+0': 'zoomReset',
        'f5': 'reload', 'ctrl+r': 'reload',
        'alt+arrowleft': 'back', 'alt+arrowright': 'forward', 'browserback': 'back', 'browserforward': 'forward',
    };

    const send = (name) => window.mvSendCommand?.(name);

    window.addEventListener('keydown', (e) => {
        const key = e.key.toLowerCase();
        if (key === 'control' || key === 'shift' || key === 'alt') return;
        const combo = (e.ctrlKey ? 'ctrl+' : '') + (e.shiftKey ? 'shift+' : '') + (e.altKey ? 'alt+' : '') + key;
        const name = commands[combo];
        if (!name) return;
        e.preventDefault();
        e.stopPropagation();
        send(name);
    }, true);

    // Ctrl+wheel zooms the document the same way the toolbar does.
    window.addEventListener('wheel', (e) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        send(e.deltaY < 0 ? 'zoomIn' : 'zoomOut');
    }, { passive: false, capture: true });

    // Mouse back/forward buttons go through the app's document history.
    const mouseNav = (e) => {
        if (e.button !== 3 && e.button !== 4) return;
        e.preventDefault();
        if (e.type === 'mouseup') send(e.button === 3 ? 'back' : 'forward');
    };
    window.addEventListener('mousedown', mouseNav, true);
    window.addEventListener('mouseup', mouseNav, true);
})();
