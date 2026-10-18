// MarkView app shell: one window holds one document (or several, as tabs). Each document has a preview
// iframe served from its own folder URL (so relative images resolve) and an optional CodeMirror editor.
(() => {
    'use strict';

    const { invoke } = window.__TAURI__.core;
    // Window-scoped: receives events sent to this window plus app-wide broadcasts (the global
    // event.listen would also receive events meant for other windows).
    const thisWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
    const listen = (name, handler) => thisWindow.listen(name, handler);
    const DOC_ORIGIN = 'http://mdfile.localhost';
    const $ = (sel) => document.querySelector(sel);

    let settings = null;
    let docs = [];
    let active = null;
    let untitledCounter = 0;
    let navCounter = 0;

    // ================================================================ helpers

    function effectiveTheme() {
        if (settings?.theme === 'light' || settings?.theme === 'dark') return settings.theme;
        return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    }

    const sameFile = (a, b) => !!a && !!b && a.toLowerCase() === b.toLowerCase();
    const fileName = (p) => p.split(/[\\/]/).pop();
    const dirName = (p) => p.slice(0, Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/')));

    function debounce(fn, ms) {
        let t = null;
        return (...args) => {
            clearTimeout(t);
            t = setTimeout(() => fn(...args), ms);
        };
    }

    function el(tag, props = {}, ...children) {
        const e = document.createElement(tag);
        for (const [k, v] of Object.entries(props)) {
            if (k === 'class') e.className = v;
            else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
            else if (v !== undefined && v !== null) e.setAttribute(k, v);
        }
        for (const c of children) if (c !== null && c !== undefined) e.append(c);
        return e;
    }

    const glyph = (code) => el('span', { class: 'glyph' }, code);

    // ================================================================ document model

    class Doc {
        constructor() {
            this.path = null;
            this.untitledId = 0;
            this.text = '';
            this.saved = '';
            this.bom = false;
            this.mode = 'view';
            this.back = [];
            this.forward = [];
            this.ready = false;
            this.editor = null;
            this.syncLine = null;
            this.renderSoon = debounce(() => this.pushRender(), 150);
            this.diskChanged = debounce(() => onDiskChanged(this), 200);
            this.depChanged = debounce(() => this.post({ type: 'refreshImages' }), 200);

            this.banner = el('div', { class: 'banner-host' });
            this.editorPane = el('div', { class: 'editor-pane' });
            this.splitter = el('div', { class: 'splitter' });
            this.iframe = el('iframe', { class: 'preview', allow: 'clipboard-write', title: 'Preview' });
            this.panes = el('div', { class: 'panes' }, this.editorPane, this.splitter, this.iframe);
            this.root = el('section', { class: 'doc' }, this.banner, this.panes);
            this.initSplitter();
            $('#docs').append(this.root);
        }

        get title() { return this.path ? fileName(this.path) : `Untitled ${this.untitledId}`; }
        get dirty() { return this.text !== this.saved; }

        post(message) {
            if (this.ready) this.iframe.contentWindow?.postMessage(message, DOC_ORIGIN);
        }

        async pushRender() {
            if (!this.ready) return;
            const html = await invoke('render', { text: this.text });
            this.post({ type: 'update', html });
        }

        async navigate(fragment) {
            this.ready = false;
            const url = await invoke('doc_url', { path: this.path, untitled: this.untitledId });
            this.iframe.src = `${url}?v=${++navCounter}${fragment ? '#' + encodeURIComponent(fragment) : ''}`;
        }

        async load(path, fragment) {
            let file;
            try {
                file = await invoke('read_file', { path });
            } catch (e) {
                showBanner(this, 'error', 'Cannot open file', `${path}\n${e}`);
                return false;
            }
            this.path = path;
            this.text = this.saved = file.text;
            this.bom = file.bom;
            hideBanner(this);
            invoke('add_recent', { path });
            await this.navigate(fragment);
            this.editor?.setText(this.text);
            return true;
        }

        setMode(mode) {
            this.mode = mode;
            const ratio = settings.splitRatio || 0.5;
            const cols = {
                view: '0 0 1fr',
                split: `${ratio}fr 6px ${1 - ratio}fr`,
                edit: '1fr 0 0',
            }[mode];
            this.panes.style.gridTemplateColumns = cols;
            this.iframe.classList.toggle('offscreen', mode === 'edit');
            if (mode === 'view') {
                this.post({ type: 'unsync' });
            } else {
                this.ensureEditor();
                requestAnimationFrame(() => this.editor?.focus());
            }
        }

        ensureEditor() {
            if (this.editor) return;
            this.editor = MVEditor.create(this.editorPane, {
                text: this.text,
                theme: effectiveTheme(),
                onChange: (text) => {
                    const wasDirty = this.dirty;
                    // CodeMirror joins lines with \n; keep the file's own line endings.
                    this.text = this.saved.includes('\r\n') ? text.replace(/\r?\n/g, '\r\n') : text;
                    this.renderSoon();
                    if (wasDirty !== this.dirty) updateChrome();
                },
                onScroll: (line) => {
                    if (this.mode === 'split') this.post({ type: 'scrollToLine', line });
                },
                onPasteImage: (file) => pasteImage(this, file),
            });
        }

        initSplitter() {
            this.splitter.addEventListener('pointerdown', (e) => {
                this.splitter.setPointerCapture(e.pointerId);
                this.panes.classList.add('dragging');
                const move = (ev) => {
                    const rect = this.panes.getBoundingClientRect();
                    const ratio = Math.min(0.85, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
                    settings.splitRatio = ratio;
                    this.panes.style.gridTemplateColumns = `${ratio}fr 6px ${1 - ratio}fr`;
                };
                const up = () => {
                    this.panes.classList.remove('dragging');
                    this.splitter.removeEventListener('pointermove', move);
                    this.splitter.removeEventListener('pointerup', up);
                    invoke('update_settings', { patch: { splitRatio: settings.splitRatio } });
                };
                this.splitter.addEventListener('pointermove', move);
                this.splitter.addEventListener('pointerup', up);
            });
        }

        destroy() {
            this.editor?.destroy();
            this.root.remove();
        }
    }

    // ================================================================ opening & closing

    async function addDoc(setup) {
        const doc = new Doc();
        docs.push(doc);
        activate(doc);
        await setup(doc);
        reportDocs();
        updateChrome();
        return doc;
    }

    async function openHere(path, fragment) {
        const existing = docs.find((d) => sameFile(d.path, path));
        if (existing) {
            activate(existing);
            if (fragment) existing.post({ type: 'scrollToFragment', id: fragment });
            return;
        }
        // Reuse an untouched empty document instead of adding a tab next to it.
        const blank = docs.length === 1 && !docs[0].path && !docs[0].text ? docs[0] : null;
        if (blank) {
            await blank.load(path, fragment);
            blank.setMode('view');
            reportDocs();
            updateChrome();
            return;
        }
        await addDoc(async (doc) => {
            doc.setMode('view');
            if (!(await doc.load(path, fragment))) {
                closeDoc(doc, true);
            }
        });
    }

    async function newHere() {
        await addDoc(async (doc) => {
            doc.untitledId = ++untitledCounter;
            await doc.navigate();
            doc.setMode('split');
        });
    }

    function activate(doc) {
        active = doc;
        for (const d of docs) d.root.classList.toggle('active', d === doc);
        updateChrome();
    }

    async function closeDoc(doc, force = false) {
        if (!force && !(await confirmDiscard(doc, 'closing'))) return false;
        const i = docs.indexOf(doc);
        docs.splice(i, 1);
        doc.destroy();
        if (active === doc) active = docs[Math.min(i, docs.length - 1)] ?? null;
        if (active) activate(active);
        reportDocs();
        updateChrome();
        return true;
    }

    async function closeWindow() {
        for (const doc of docs.filter((d) => d.dirty)) {
            activate(doc);
            if (!(await confirmDiscard(doc, 'closing'))) return;
        }
        invoke('close_window');
    }

    function reportDocs() {
        invoke('report_docs', { paths: docs.filter((d) => d.path).map((d) => d.path) });
    }

    async function confirmDiscard(doc, action) {
        if (!doc.dirty) return true;
        activate(doc);
        const choice = await showDialog({
            title: 'Unsaved changes',
            content: `Save changes to "${doc.title}" before ${action}?`,
            primary: 'Save',
            secondary: "Don't save",
            close: 'Cancel',
        });
        if (choice === 'primary') return save(doc);
        return choice === 'secondary';
    }

    async function navigateTo(doc, path, fragment) {
        if (sameFile(doc.path, path)) {
            if (fragment) doc.post({ type: 'scrollToFragment', id: fragment });
            return;
        }
        if (!(await confirmDiscard(doc, 'leaving it'))) return;
        // Already open in another tab of this window: switch to it.
        const other = docs.find((d) => d !== doc && sameFile(d.path, path));
        if (other) {
            activate(other);
            return;
        }
        if (doc.path) doc.back.push(doc.path);
        doc.forward = [];
        await doc.load(path, fragment);
        reportDocs();
        updateChrome();
    }

    async function goHistory(doc, direction) {
        const from = direction < 0 ? doc.back : doc.forward;
        const to = direction < 0 ? doc.forward : doc.back;
        if (!from.length || !(await confirmDiscard(doc, 'leaving it'))) return;
        if (doc.path) to.push(doc.path);
        await doc.load(from.pop());
        reportDocs();
        updateChrome();
    }

    // ================================================================ saving

    async function save(doc) {
        if (!doc.path) return saveAs(doc);
        try {
            await invoke('write_file', { path: doc.path, text: doc.text, bom: doc.bom });
            doc.saved = doc.text;
            updateChrome();
            return true;
        } catch (e) {
            showBanner(doc, 'error', 'Save failed', String(e));
            return false;
        }
    }

    async function saveAs(doc) {
        const suggested = doc.path ? fileName(doc.path) : (await invoke('suggest_name', { text: doc.text })) + '.md';
        const path = await invoke('pick_save', { suggested, kind: 'md', directory: doc.path ? dirName(doc.path) : null });
        if (!path) return false;
        const previous = doc.path;
        doc.path = path;
        if (!(await save(doc))) {
            doc.path = previous;
            return false;
        }
        invoke('add_recent', { path });
        await doc.navigate(); // relative image paths now resolve against the new folder
        reportDocs();
        updateChrome();
        return true;
    }

    async function pasteImage(doc, file) {
        if (!doc.path) {
            showBanner(doc, 'info', 'Save the document first',
                'Pasted images are stored in an images folder next to the document, so it needs to be saved once.');
            return;
        }
        const ext = { 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp', 'image/bmp': 'bmp', 'image/svg+xml': 'svg' }[file.type] ?? 'png';
        const data = await new Promise((resolve) => {
            const reader = new FileReader();
            reader.onload = () => resolve(String(reader.result).split(',')[1]);
            reader.readAsDataURL(file);
        });
        try {
            const rel = await invoke('save_pasted_image', { docPath: doc.path, data, ext });
            doc.editor.insert(`![image](${rel})`);
        } catch (e) {
            showBanner(doc, 'error', 'Could not save pasted image', String(e));
        }
    }

    // ================================================================ disk changes

    async function onDiskChanged(doc) {
        if (!doc.path || !docs.includes(doc)) return;
        let file;
        try {
            file = await invoke('read_file', { path: doc.path });
        } catch {
            if (!(await invoke('path_exists', { path: doc.path }))) {
                showBanner(doc, 'warning', 'File deleted or moved', 'The file is no longer on disk. Save to recreate it.');
                doc.saved = '\u0000';
                updateChrome();
            }
            return; // otherwise still being written; another change event follows
        }
        if (file.text === doc.saved) return; // our own save, or a touch without content change
        const reload = () => {
            doc.text = doc.saved = file.text;
            doc.bom = file.bom;
            doc.editor?.setText(doc.text);
            doc.pushRender();
            updateChrome();
        };
        if (!doc.dirty) {
            reload();
        } else {
            showBanner(doc, 'warning', 'File changed on disk',
                'Another program modified this file. Reload it and lose your edits?', 'Reload', reload);
        }
    }

    // ================================================================ preview messages

    window.addEventListener('message', async (e) => {
        if (e.origin !== DOC_ORIGIN) return;
        const doc = docs.find((d) => d.iframe.contentWindow === e.source);
        if (!doc) return;
        const m = e.data;
        switch (m.type) {
            case 'ready':
                doc.ready = true;
                doc.post({ type: 'theme', theme: effectiveTheme() });
                // The page was rendered from disk; catch up with unsaved edits.
                if (doc.dirty || !doc.path) doc.pushRender();
                if (doc.mode === 'split' && doc.editor) doc.post({ type: 'scrollToLine', line: doc.editor.topLine() });
                break;
            case 'command':
                runCommand(m.name);
                break;
            case 'link':
                await followLink(doc, m.url, m.newWindow);
                break;
            case 'toggleTask':
                toggleTask(doc, m.line, m.checked);
                break;
            case 'reveal':
                if (doc.mode !== 'view') doc.editor?.revealLine(m.line);
                break;
            case 'localFiles':
                if (doc.path) invoke('watch_dependencies', { doc: doc.path, urls: m.urls });
                break;
        }
    });

    async function followLink(doc, url, newWindow) {
        const result = await invoke('open_link', { url });
        if (result.kind === 'missing') {
            showBanner(doc, 'warning', 'Link target not found', result.path);
        } else if (result.kind === 'markdown') {
            if (newWindow) await invoke('open_paths', { paths: [result.path], fragment: result.fragment });
            else await navigateTo(doc, result.path, result.fragment);
        }
    }

    const TASK = /^(\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+\[)([ xX])(\])/;

    function toggleTask(doc, line, checked) {
        const lines = doc.text.split('\n');
        if (line < 0 || line >= lines.length) return;
        const updated = lines[line].replace(TASK, (_, a, _b, c) => a + (checked ? 'x' : ' ') + c);
        if (updated === lines[line]) return;
        lines[line] = updated;
        doc.text = lines.join('\n');
        doc.editor?.replaceLine(line, updated.replace(/\r$/, ''));
        updateChrome();
    }

    // ================================================================ commands

    async function runCommand(name) {
        const doc = active;
        try {
            switch (name) {
                case 'open': {
                    const paths = await invoke('pick_open');
                    if (paths.length) await invoke('open_paths', { paths, fragment: null });
                    break;
                }
                case 'new': await invoke('new_document'); break;
                case 'save': if (doc) await save(doc); break;
                case 'saveAs': if (doc) await saveAs(doc); break;
                case 'closeDoc':
                    // Without tabs a window is one document: closing it closes the window.
                    if (!settings.useTabs && docs.length <= 1) await closeWindow();
                    else if (doc) await closeDoc(doc);
                    break;
                case 'nextTab': cycleTab(1); break;
                case 'prevTab': cycleTab(-1); break;
                case 'toggleToc': await invoke('update_settings', { patch: { tocVisible: !settings.tocVisible } }); break;
                case 'cycleMode': if (doc) setMode(doc, { view: 'split', split: 'edit', edit: 'view' }[doc.mode]); break;
                case 'modeView': if (doc) setMode(doc, 'view'); break;
                case 'modeSplit': if (doc) setMode(doc, 'split'); break;
                case 'modeEdit': if (doc) setMode(doc, 'edit'); break;
                case 'print': if (doc) doc.post({ type: 'print' }); break;
                case 'exportPdf': if (doc) await exportPdf(doc); break;
                case 'zoomIn': await setZoom(settings.zoom + 0.1); break;
                case 'zoomOut': await setZoom(settings.zoom - 0.1); break;
                case 'zoomReset': await setZoom(1); break;
                case 'reload': if (doc) await reload(doc); break;
                case 'back': if (doc) await goHistory(doc, -1); break;
                case 'forward': if (doc) await goHistory(doc, 1); break;
            }
        } catch (e) {
            console.error(`Command ${name} failed`, e);
        }
    }
    window.mvSendCommand = runCommand;

    function setMode(doc, mode) {
        doc.setMode(mode);
        updateChrome();
    }

    function cycleTab(delta) {
        if (docs.length < 2) return;
        const i = docs.indexOf(active);
        activate(docs[(i + delta + docs.length) % docs.length]);
    }

    async function setZoom(zoom) {
        zoom = Math.round(Math.min(3, Math.max(0.5, zoom)) * 100) / 100;
        await invoke('update_settings', { patch: { zoom } });
    }

    async function reload(doc) {
        if (doc.path && !doc.dirty) {
            try {
                const file = await invoke('read_file', { path: doc.path });
                doc.text = doc.saved = file.text;
                doc.bom = file.bom;
                doc.editor?.setText(doc.text);
            } catch (e) {
                showBanner(doc, 'error', 'Cannot reload', String(e));
            }
        }
        await doc.navigate();
        updateChrome();
    }

    async function exportPdf(doc) {
        if (!doc.path) {
            showBanner(doc, 'info', 'Save the document first', 'PDF export needs the document saved so its images can be found.');
            return;
        }
        const suggested = fileName(doc.path).replace(/\.[^.]+$/, '') + '.pdf';
        const out = await invoke('pick_save', { suggested, kind: 'pdf', directory: dirName(doc.path) });
        if (!out) return;
        showBanner(doc, 'info', 'Exporting PDF...', out);
        try {
            await invoke('export_pdf', { docPath: doc.path, text: doc.text, out });
            showBanner(doc, 'success', 'PDF exported', out, 'Open', () => invoke('open_path_external', { path: out }));
        } catch (e) {
            showBanner(doc, 'error', 'PDF export failed', String(e));
        }
    }

    // ================================================================ chrome

    function updateChrome() {
        const doc = active;
        const has = !!doc;

        $('#docs').hidden = !has;
        $('#empty').hidden = has;
        if (!has) buildRecentList();

        $('#btn-back').disabled = !doc?.back.length;
        $('#btn-forward').disabled = !doc?.forward.length;
        $('#btn-save').disabled = !has;
        for (const [id, mode] of [['#btn-view', 'view'], ['#btn-split', 'split'], ['#btn-edit', 'edit']]) {
            $(id).disabled = !has;
            $(id).classList.toggle('checked', doc?.mode === mode);
        }
        $('#btn-toc').classList.toggle('checked', !!settings?.tocVisible);
        $('#btn-zoom').textContent = `${Math.round((settings?.zoom ?? 1) * 100)}%`;
        $('#status').replaceChildren(el('bdi', {}, doc ? (doc.path ?? 'Unsaved document') : ''));

        const title = doc ? `${doc.dirty ? '● ' : ''}${doc.title} - MarkView` : 'MarkView';
        if (document.title !== title) {
            document.title = title;
            invoke('set_title', { title });
        }
        renderTabs();
    }

    function renderTabs() {
        const show = !!settings?.useTabs || docs.length > 1;
        $('#tabstrip').hidden = !show;
        if (!show) return;
        $('#tabs').replaceChildren(...docs.map((doc) => {
            const close = el('button', { class: 'close', title: 'Close (Ctrl+W)' }, glyph(''));
            close.addEventListener('click', (e) => {
                e.stopPropagation();
                closeDoc(doc);
            });
            const tab = el('div', {
                class: 'tab' + (doc === active ? ' selected' : ''),
                role: 'tab',
                title: doc.path ?? doc.title,
                onclick: () => activate(doc),
                onauxclick: (e) => { if (e.button === 1) closeDoc(doc); },
            }, glyph(''), el('span', { class: 'title' }, doc.title + (doc.dirty ? ' ●' : '')), close);
            return tab;
        }));
    }

    async function buildRecentList() {
        const recent = settings?.recent ?? [];
        const existing = [];
        for (const p of recent.slice(0, 15)) {
            if (existing.length >= 10) break;
            if (await invoke('path_exists', { path: p })) existing.push(p);
        }
        $('#recent-header').hidden = existing.length === 0;
        $('#recent-list').replaceChildren(...existing.map((p) =>
            el('button', { class: 'recent-item', title: p, onclick: () => invoke('open_paths', { paths: [p], fragment: null }) },
                fileName(p), el('span', { class: 'dir' }, dirName(p)))));
    }

    // ================================================================ info bar

    function showBanner(doc, severity, title, message, actionText, action) {
        const icons = { info: '', warning: '', error: '', success: '' };
        const actions = el('div', { class: 'actions' });
        if (actionText && action) {
            actions.append(el('button', { class: 'btn', onclick: () => { hideBanner(doc); action(); } }, actionText));
        }
        actions.append(el('button', { class: 'icon-btn', title: 'Close', onclick: () => hideBanner(doc) }, glyph('')));
        doc.banner.replaceChildren(el('div', { class: `infobar ${severity}` },
            el('span', { class: 'icon glyph' }, icons[severity] ?? icons.info),
            el('div', { class: 'text' }, el('strong', {}, title), message ?? ''),
            actions));
    }

    function hideBanner(doc) {
        doc.banner.replaceChildren();
    }

    // ================================================================ flyout menus

    const menu = $('#menu');
    let menuAnchor = null;

    function showMenu(anchor, items) {
        menu.replaceChildren(...items.map((item) => {
            if (item.separator) return el('div', { class: 'menu-sep' });
            const button = el('button', { class: 'menu-item', role: 'menuitem', title: item.tooltip },
                el('span', { class: 'check glyph' }, item.checked ? (item.radio ? '' : '') : ''),
                el('span', { class: 'label' }, item.label),
                item.shortcut ? el('span', { class: 'shortcut' }, item.shortcut) : null);
            button.disabled = !!item.disabled;
            button.addEventListener('click', () => {
                hideMenu();
                item.action?.();
            });
            return button;
        }));
        menu.hidden = false;
        menuAnchor = anchor;
        const r = anchor.getBoundingClientRect();
        const w = menu.offsetWidth;
        menu.style.left = `${Math.max(4, Math.min(r.left, innerWidth - w - 4))}px`;
        menu.style.top = `${r.bottom + 4}px`;
    }

    function hideMenu() {
        menu.hidden = true;
        menuAnchor = null;
    }

    document.addEventListener('pointerdown', (e) => {
        if (!menu.hidden && !menu.contains(e.target) && !menuAnchor?.contains(e.target)) hideMenu();
    }, true);
    window.addEventListener('blur', hideMenu); // clicks into the preview iframe
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') hideMenu(); });

    async function recentMenu(anchor) {
        const recent = settings.recent ?? [];
        if (!recent.length) {
            showMenu(anchor, [{ label: 'No recent files', disabled: true }]);
            return;
        }
        const exists = await Promise.all(recent.map((p) => invoke('path_exists', { path: p })));
        showMenu(anchor, [
            ...recent.map((p, i) => ({
                label: fileName(p),
                tooltip: p,
                disabled: !exists[i],
                action: () => invoke('open_paths', { paths: [p], fragment: null }),
            })),
            { separator: true },
            { label: 'Clear recent files', action: () => invoke('update_settings', { patch: { recent: [] } }) },
        ]);
    }

    function themeMenu(anchor) {
        const pick = (theme) => () => invoke('update_settings', { patch: { theme } });
        showMenu(anchor, [
            { label: 'Use system setting', radio: true, checked: settings.theme === 'system', action: pick('system') },
            { label: 'Light', radio: true, checked: settings.theme === 'light', action: pick('light') },
            { label: 'Dark', radio: true, checked: settings.theme === 'dark', action: pick('dark') },
        ]);
    }

    async function moreMenu(anchor) {
        const doc = active;
        const assoc = await invoke('association_status');
        const assocLabel = assoc.registered
            ? 'Re-register / set as default .md app...'
            : assoc.registeredExe
                ? 'Re-register .md files here (MarkView was moved)...'
                : 'Use MarkView for .md files...';
        showMenu(anchor, [
            { label: 'Save as...', shortcut: 'Ctrl+Shift+S', disabled: !doc, action: () => runCommand('saveAs') },
            { label: 'Reload', shortcut: 'F5', disabled: !doc, action: () => runCommand('reload') },
            { separator: true },
            { label: 'Show in Explorer', disabled: !doc?.path, action: () => invoke('reveal', { path: doc.path }) },
            { label: 'Copy file path', disabled: !doc?.path, action: () => navigator.clipboard.writeText(doc.path) },
            { separator: true },
            {
                label: 'Open files in tabs',
                checked: settings.useTabs,
                action: () => invoke('update_settings', { patch: { useTabs: !settings.useTabs } }),
            },
            { separator: true },
            // The Store build's association comes from its package; Windows Settings picks the default.
            assoc.packaged
                ? { label: 'Set as default .md app...', action: () => invoke('open_default_apps') }
                : { label: assocLabel, action: () => registerAssociation(assoc.registeredExe) },
            assoc.registeredExe ? { label: 'Remove .md file association', action: removeAssociation } : null,
            { separator: true },
            { label: 'Keyboard shortcuts', action: showShortcuts },
            { label: 'About MarkView', action: showAbout },
        ].filter(Boolean));
    }

    // Where the exe lives, and why it has to stay there for "Open with" to keep working.
    function locationNote(info) {
        const parts = [
            el('p', {}, 'MarkView is running from:'),
            // Line breaks only after path separators, never inside a folder or file name.
            el('div', { class: 'path-box' }, ...info.exe.split('\\').flatMap((part, i, all) =>
                i < all.length - 1 ? [part + '\\', el('wbr')] : [part])),
            el('button', { class: 'link-btn', onclick: () => invoke('reveal', { path: info.exe }) }, 'Show in Explorer'),
            el('p', {}, 'Keep MarkView.exe in this folder. Windows opens .md files with the program at this exact ' +
                'location, so if it is moved or deleted, double-clicking .md files stops working. ' +
                '(If you move it, MarkView will offer to set this up again the next time you open it.)'),
        ];
        if (info.previous && info.previous.toLowerCase() !== info.exe.toLowerCase()) {
            parts.push(el('p', { class: 'note-warning' },
                `.md files are currently set to open with a MarkView at a different location:\n${info.previous}\n` +
                'Setting it up here replaces that.'));
        }
        if (info.temporary) {
            parts.push(el('p', { class: 'note-warning' },
                'This looks like a temporary folder (Downloads or Temp). Consider choosing "No" for now, ' +
                `moving MarkView.exe to a permanent folder such as ${info.suggested}, and opening it from there.`));
        }
        return parts;
    }

    async function setUpAssociation() {
        try {
            await invoke('register_association');
        } catch (e) {
            await showDialog({ title: 'Could not set up file association', content: String(e), close: 'OK' });
            return false;
        }
        invoke('open_default_apps');
        return true;
    }

    // First window of a launch: offer to become the .md app (unless already set up or declined for good).
    async function offerAssociation() {
        const info = await invoke('association_prompt');
        if (!info) return;
        const choice = await showDialog({
            title: 'Open .md files with MarkView?',
            content: el('div', {},
                el('p', {}, 'MarkView can be your app for Markdown files, so double-clicking a .md file opens it here.'),
                ...locationNote(info),
                el('p', { class: 'secondary-text' }, 'If you choose Yes, Windows Settings opens next so you can confirm ' +
                    'MarkView as the default. Windows requires that step; apps cannot set it themselves.')),
            primary: 'Yes',
            secondary: 'No',
            close: "No, don't ask again",
            escape: 'secondary',
        });
        if (choice === 'primary') await setUpAssociation();
        else if (choice === 'close') await invoke('update_settings', { patch: { assocPrompt: 'never' } });
    }

    // From the "..." menu: register, or re-register from this location (e.g. after moving the exe).
    async function registerAssociation(previous) {
        const info = await invoke('app_info');
        const choice = await showDialog({
            title: 'Open .md files with MarkView',
            content: el('div', {},
                ...locationNote({ exe: info.exe, previous, temporary: info.temporary, suggested: info.suggested }),
                el('p', { class: 'secondary-text' }, 'Windows Settings opens next so you can confirm MarkView as the default.')),
            primary: 'Set up',
            close: 'Cancel',
        });
        if (choice === 'primary') await setUpAssociation();
    }

    async function removeAssociation() {
        const choice = await showDialog({
            title: 'Remove .md file association?',
            content: 'MarkView will no longer appear under "Open with" for Markdown files. ' +
                'If it is your default .md app, Windows will ask which app to use next time you open one.',
            primary: 'Remove',
            close: 'Cancel',
        });
        if (choice !== 'primary') return;
        try {
            await invoke('unregister_association');
        } catch (e) {
            await showDialog({ title: 'Could not remove file association', content: String(e), close: 'OK' });
        }
    }

    function showShortcuts() {
        const rows = [
            ['Ctrl+O', 'Open file'], ['Ctrl+N', 'New document'], ['Ctrl+S', 'Save'], ['Ctrl+Shift+S', 'Save as'],
            ['Ctrl+W', 'Close document'], ['Ctrl+Tab', 'Next tab (when tabs are on)'],
            ['Ctrl+1 / 2 / 3', 'Read / Split / Edit'], ['Ctrl+E', 'Cycle view mode'], ['Ctrl+T', 'Table of contents'],
            ['Ctrl+F', 'Find (in the editor)'], ['Ctrl+= / - / 0', 'Zoom in / out / reset'], ['F5', 'Reload'],
            ['Alt+Left / Right', 'Back / Forward'], ['Ctrl+P', 'Print'],
        ];
        const table = el('table', {}, ...rows.map(([k, v]) => el('tr', {}, el('td', {}, k), el('td', {}, v))));
        const note = el('p', {}, 'Double-click the preview to jump to that line in the editor. ' +
            'Paste or drop images into the editor to add them to the document.');
        showDialog({ title: 'Keyboard shortcuts', content: el('div', {}, table, note), close: 'Close' });
    }

    async function showAbout() {
        const info = await invoke('app_info');
        showDialog({
            title: 'About MarkView',
            content: el('div', {},
                el('p', {}, `MarkView ${info.version}\nA Markdown viewer and editor.`),
                el('button', { class: 'link-btn', onclick: () => invoke('open_link', { url: info.repository }) },
                    info.repository.replace(/^https:\/\//, '')),
                el('p', {}, 'Built with Rust, Tauri, WebView2, comrak, CodeMirror, highlight.js, Mermaid, KaTeX and github-markdown-css.'),
                el('p', {}, `Settings: ${info.dataDir}`)),
            close: 'Close',
        });
    }

    // ================================================================ content dialog

    let dialogOpen = false;

    // Resolves to 'primary' | 'secondary' | 'close'. `escape` picks the result for the Esc key.
    function showDialog({ title, content, primary, secondary, close, escape = 'close' }) {
        if (dialogOpen) return Promise.resolve('close');
        dialogOpen = true;
        hideMenu();
        const layer = $('#dialog-layer');
        layer.querySelector('.dialog-title').textContent = title;
        layer.querySelector('.dialog-content').replaceChildren(content);
        const footer = layer.querySelector('.dialog-footer');
        return new Promise((resolve) => {
            const done = (result) => {
                layer.hidden = true;
                dialogOpen = false;
                document.removeEventListener('keydown', onKey, true);
                resolve(result);
            };
            const onKey = (e) => {
                if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(escape); }
                if (e.key === 'Enter' && primary) { e.preventDefault(); e.stopPropagation(); done('primary'); }
            };
            const buttons = [];
            if (primary) buttons.push(el('button', { class: 'btn accent', onclick: () => done('primary') }, primary));
            if (secondary) buttons.push(el('button', { class: 'btn', onclick: () => done('secondary') }, secondary));
            if (close) buttons.push(el('button', { class: 'btn', onclick: () => done('close') }, close));
            footer.replaceChildren(...buttons);
            layer.hidden = false;
            document.addEventListener('keydown', onKey, true);
            buttons[0]?.focus();
        });
    }

    // ================================================================ settings

    function applySettings() {
        const theme = effectiveTheme();
        document.documentElement.classList.toggle('dark', theme === 'dark');
        for (const doc of docs) {
            doc.post({ type: 'theme', theme });
            doc.post({ type: 'toc', visible: settings.tocVisible });
            doc.post({ type: 'zoom', zoom: settings.zoom });
            doc.editor?.setTheme(theme);
        }
        updateChrome();
    }

    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => settings && applySettings());

    // ================================================================ drag & drop (files from Explorer)

    async function onDrop(paths, position) {
        const x = position.x / devicePixelRatio;
        const y = position.y / devicePixelRatio;
        const doc = active;
        const target = document.elementFromPoint(x, y);
        const overEditor = doc?.editor && doc.mode !== 'view' && doc.editorPane.contains(target);
        const markdown = paths.filter((p) => /\.(md|markdown|mdown|mkdn?|mdwn|mdtxt|mdtext|txt)$/i.test(p));
        if (overEditor) {
            const links = [];
            for (const p of paths) {
                if (markdown.includes(p)) continue;
                links.push(await invoke('link_for_file', { docPath: doc.path, file: p }));
            }
            if (links.length) {
                doc.editor.moveCursorToPoint(x, y);
                doc.editor.insert(links.join('\n'));
            }
        }
        if (markdown.length) await invoke('open_paths', { paths: markdown, fragment: null });
    }

    // ================================================================ wiring

    document.querySelectorAll('[data-cmd]').forEach((b) => b.addEventListener('click', () => runCommand(b.dataset.cmd)));
    document.querySelectorAll('[data-menu]').forEach((b) => b.addEventListener('click', () => {
        if (menuAnchor === b) return hideMenu();
        ({ recent: recentMenu, theme: themeMenu, more: moreMenu })[b.dataset.menu](b);
    }));
    $('#add-tab').addEventListener('click', () => newHere());
    // Middle-click / mouse back on chrome.
    document.addEventListener('contextmenu', (e) => {
        if (!e.target.closest('.cm-editor')) e.preventDefault();
    });

    async function handleOpenRequest(request) {
        if (request.kind === 'open') await openHere(request.path, request.fragment);
        else if (request.kind === 'new') await newHere();
    }

    async function start() {
        settings = await invoke('get_settings');
        const accent = await invoke('accent_colors').catch(() => null);
        if (accent) {
            document.documentElement.style.setProperty('--accent-light', accent.light);
            document.documentElement.style.setProperty('--accent-dark', accent.dark);
        }
        applySettings();

        await listen('open-request', (e) => handleOpenRequest(e.payload));
        await listen('close-requested', () => closeWindow());
        await listen('settings-changed', (e) => {
            settings = e.payload;
            applySettings();
        });
        await listen('doc-changed', (e) => docs.filter((d) => sameFile(d.path, e.payload.path)).forEach((d) => d.diskChanged()));
        await listen('dep-changed', (e) => docs.filter((d) => sameFile(d.path, e.payload.path)).forEach((d) => d.depChanged()));
        await window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((e) => {
            if (e.payload.type === 'drop') onDrop(e.payload.paths, e.payload.position);
        });

        const requests = await invoke('window_ready');
        for (const r of requests) await handleOpenRequest(r);
        updateChrome();
        offerAssociation();
    }

    start();
})();
