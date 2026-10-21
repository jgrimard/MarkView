// Preview page: runs inside the app's iframe (origin http://mdfile.localhost), or standalone for PDF export.
(() => {
    'use strict';

    const APP = 'http://mdfile.localhost/__app/';
    const SHELL_ORIGIN = 'http://tauri.localhost';
    const config = JSON.parse(document.getElementById('mv-config').textContent);
    const article = document.getElementById('article');
    const tocList = document.getElementById('toc-list');
    const lightbox = document.getElementById('lightbox');
    const embedded = window.parent !== window;
    const post = (msg) => { if (embedded) window.parent.postMessage(msg, SHELL_ORIGIN); };

    window.mvSendCommand = (name) => post({ type: 'command', name });

    let theme = config.theme;
    let strings = config.strings ?? {};
    // A preview string, with {name} placeholders filled in.
    const t = (key, vars = {}) => (strings[key] ?? key).replace(/\{(\w+)\}/g, (m, k) => vars[k] ?? m);
    let generation = 0;
    let headings = [];
    let syncedLine = null;

    hljs.configure({ ignoreUnescapedHTML: true });

    // 0-based source line of an element rendered with data-sourcepos="line:col-line:col".
    const lineOf = (el) => parseInt(el.dataset.sourcepos, 10) - 1;

    // ---------- large documents (LARGE_DOC_ELEMENTS in render.rs; the server sets the class on load) ----------
    const LARGE_DOC_ELEMENTS = 5000;
    const updateLarge = () => article.classList.toggle('large',
        article.querySelectorAll('[data-sourcepos]').length > LARGE_DOC_ELEMENTS);

    // In a large document, math and code highlighting are done as their block nears the screen.
    // Printing and PDF export (standalone) still get everything.
    const nearWork = new Map(); // top-level block -> pending functions
    const nearObserver = new IntersectionObserver((entries) => {
        for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            nearObserver.unobserve(entry.target);
            const work = nearWork.get(entry.target) ?? [];
            nearWork.delete(entry.target);
            work.forEach((fn) => fn());
            if (work.length) refreshFindSoon(); // highlighting replaced text that matches pointed into
        }
    }, { rootMargin: '2000px 0px' });
    function whenNear(el, fn) {
        if (!embedded || !article.classList.contains('large')) return fn();
        let block = el;
        while (block.parentElement && block.parentElement !== article) block = block.parentElement;
        if (!nearWork.has(block)) {
            nearWork.set(block, []);
            nearObserver.observe(block);
        }
        nearWork.get(block).push(fn);
    }
    function flushNear() {
        nearObserver.disconnect();
        const work = [...nearWork.values()].flat();
        nearWork.clear();
        work.forEach((fn) => fn());
    }

    // ---------- lazy script loading (Mermaid and KaTeX are only fetched when a page needs them) ----------
    const loaded = new Map();
    function loadScript(path) {
        if (!loaded.has(path)) {
            loaded.set(path, new Promise((resolve, reject) => {
                const s = document.createElement('script');
                s.src = APP + path;
                s.onload = resolve;
                s.onerror = () => reject(new Error('Failed to load ' + path));
                document.head.appendChild(s);
            }));
        }
        return loaded.get(path);
    }

    // ---------- theme / zoom / toc ----------
    function applyTheme(next) {
        theme = next;
        document.documentElement.dataset.theme = next;
        const dark = next === 'dark';
        const use = (id, on) => { document.getElementById(id).media = on ? 'all' : 'not all'; };
        use('md-light', !dark);
        use('hl-light', !dark);
        use('md-dark', dark);
        use('hl-dark', dark);
        return renderMermaid(generation);
    }

    const applyZoom = (zoom) => { article.style.zoom = String(zoom); };

    function applyStrings(next) {
        strings = next;
        document.querySelector('#toc .toc-title').textContent = t('contents');
        document.getElementById('toc').setAttribute('aria-label', t('contents'));
        article.querySelectorAll('.code-copy-btn:not(.copied)').forEach((b) => { b.textContent = t('copy'); });
        findInput.placeholder = t('find');
        findInput.setAttribute('aria-label', t('find'));
        for (const [id, key] of [['find-prev', 'findPrev'], ['find-next', 'findNext'], ['find-close', 'findClose']]) {
            const button = document.getElementById(id);
            button.title = t(key);
            button.setAttribute('aria-label', t(key));
        }
        showCount();
    }
    const setToc = (visible) => document.body.classList.toggle('toc-hidden', !visible);

    // ---------- enhancements applied after every render ----------
    function enhance() {
        const gen = ++generation;
        nearObserver.disconnect(); // the previous content is gone
        nearWork.clear();
        convertMermaidBlocks();
        highlightCode();
        addCopyButtons();
        enableTaskLists();
        buildToc();
        reportLocalFiles();
        const pending = [renderMath(), renderMermaid(gen)];
        if (syncedLine !== null) scrollToLine(syncedLine);
        return Promise.all(pending);
    }

    // ```mermaid fences arrive as <pre><code class="language-mermaid">; Mermaid wants a div.
    function convertMermaidBlocks() {
        article.querySelectorAll('pre > code.language-mermaid').forEach((code) => {
            const pre = code.parentElement;
            const div = document.createElement('div');
            div.className = 'mermaid';
            if (pre.dataset.sourcepos) div.dataset.sourcepos = pre.dataset.sourcepos;
            div.textContent = code.textContent;
            pre.replaceWith(div);
        });
    }

    function highlightCode() {
        article.querySelectorAll('pre > code').forEach((code) => {
            const langClass = [...code.classList].find((c) => c.startsWith('language-'));
            if (!langClass || code.dataset.mathStyle) return;
            const lang = langClass.slice(9);
            const pre = code.parentElement;
            if (lang && !pre.querySelector('.code-lang')) {
                const label = document.createElement('span');
                label.className = 'code-lang';
                label.textContent = lang;
                pre.appendChild(label);
                pre.classList.add('has-lang');
            }
            if (hljs.getLanguage(lang)) whenNear(code, () => hljs.highlightElement(code));
        });
    }

    async function copyText(text) {
        try {
            await navigator.clipboard.writeText(text);
            return true;
        } catch {
            const area = document.createElement('textarea');
            area.value = text;
            area.style.position = 'fixed';
            area.style.opacity = '0';
            document.body.appendChild(area);
            area.select();
            const ok = document.execCommand('copy');
            area.remove();
            return ok;
        }
    }

    function addCopyButtons() {
        article.querySelectorAll('pre').forEach((pre) => {
            const code = pre.querySelector('code');
            if (!code || pre.querySelector('.code-copy-btn')) return;
            const button = document.createElement('button');
            button.type = 'button';
            button.className = 'code-copy-btn';
            button.textContent = t('copy');
            button.addEventListener('click', async (e) => {
                e.stopPropagation();
                const ok = await copyText(code.innerText);
                button.textContent = ok ? t('copied') : t('copyFailed');
                button.classList.toggle('copied', ok);
                setTimeout(() => {
                    button.textContent = t('copy');
                    button.classList.remove('copied');
                }, 1500);
            });
            pre.appendChild(button);
        });
    }

    // Checking a task box edits the Markdown source ("- [ ]" <-> "- [x]").
    function enableTaskLists() {
        article.querySelectorAll('li.task-list-item input[type=checkbox]').forEach((cb) => {
            cb.disabled = !embedded;
            cb.addEventListener('click', () => {
                const owner = cb.closest('li[data-sourcepos]');
                if (owner) post({ type: 'toggleTask', line: lineOf(owner), checked: cb.checked });
            });
        });
    }

    function buildToc() {
        headings = [...article.querySelectorAll('h1[id], h2[id], h3[id], h4[id]')];
        tocList.replaceChildren(...headings.map((h) => {
            const li = document.createElement('li');
            li.className = 'level-' + h.tagName.substring(1);
            const a = document.createElement('a');
            a.href = '#' + h.id;
            a.textContent = h.textContent.trim();
            a.title = a.textContent;
            a.addEventListener('click', (e) => {
                e.preventDefault();
                e.stopPropagation();
                h.scrollIntoView({ behavior: 'smooth', block: 'start' });
            });
            li.appendChild(a);
            return li;
        }));
        document.body.classList.toggle('toc-empty', headings.length < 2);
        updateActiveHeading();
    }

    function updateActiveHeading() {
        let active = null;
        for (const h of headings) {
            if (h.getBoundingClientRect().top <= 90) active = h;
            else break;
        }
        tocList.querySelectorAll('a.active').forEach((a) => a.classList.remove('active'));
        if (active) {
            const link = tocList.querySelector(`a[href="#${CSS.escape(active.id)}"]`);
            if (link) {
                link.classList.add('active');
                link.scrollIntoView({ block: 'nearest' });
            }
        }
    }

    // comrak renders $x$ / $$x$$ / ```math as elements with data-math-style="inline|display".
    async function renderMath() {
        const nodes = [...article.querySelectorAll('[data-math-style]')].filter((el) => !el.dataset.rendered);
        if (!nodes.length) return;
        try {
            await loadScript('vendor/katex/katex.min.js');
        } catch (e) {
            console.error(e);
            return;
        }
        for (const el of nodes) whenNear(el, () => renderMathNode(el));
    }

    function renderMathNode(el) {
        if (el.dataset.rendered || !el.isConnected) return;
        const display = el.dataset.mathStyle === 'display';
        let target = el;
        if (el.tagName === 'CODE' && el.parentElement?.tagName === 'PRE') {
            target = document.createElement('div');
            target.className = 'math-display';
            if (el.parentElement.dataset.sourcepos) target.dataset.sourcepos = el.parentElement.dataset.sourcepos;
            el.parentElement.replaceWith(target);
        }
        katex.render(el.textContent, target, { displayMode: display, throwOnError: false });
        target.dataset.rendered = '1';
        el.dataset.rendered = '1';
    }

    // Rendered diagrams are cached by theme + source so live editing does not re-render unchanged ones.
    const mermaidCache = new Map();
    let mermaidTheme = null;
    let mermaidSeq = 0;
    async function renderMermaid(gen) {
        const nodes = [...article.querySelectorAll('.mermaid')];
        if (!nodes.length) return;
        try {
            await loadScript('vendor/mermaid/mermaid.min.js');
        } catch (e) {
            console.error(e);
            return;
        }
        if (mermaidTheme !== theme) {
            mermaid.initialize({
                startOnLoad: false,
                theme: theme === 'dark' ? 'dark' : 'default',
                securityLevel: 'strict',
                // Mermaid's Gantt text (11px, 10px axis) is small next to the other diagrams.
                gantt: { fontSize: 14, sectionFontSize: 14, barHeight: 26, barGap: 6, leftPadding: 90 },
                themeCSS: '.grid .tick text { font-size: 11px; }',
            });
            mermaidTheme = theme;
        }
        for (const node of nodes) {
            if (gen !== generation) return; // a newer render replaced the content
            const source = node.dataset.src ?? node.textContent;
            node.dataset.src = source;
            const key = theme + '\n' + source;
            if (!mermaidCache.has(key)) {
                const id = 'mmd-' + (++mermaidSeq);
                // Draw at the width the diagram will be shown at: Gantt charts fill their container, and Mermaid's
                // default container (the whole page) would make them shrink, text and all, to fit the article.
                const style = getComputedStyle(node);
                const width = node.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
                const host = width > 0 ? document.createElement('div') : null;
                if (host) {
                    host.style.cssText = `position: absolute; top: 0; left: -10000px; width: ${width}px;`;
                    document.body.appendChild(host);
                }
                try {
                    const { svg } = await mermaid.render(id, source, host ?? undefined);
                    mermaidCache.set(key, { svg });
                } catch (e) {
                    mermaidCache.set(key, { error: String(e?.message ?? e) });
                    document.getElementById('d' + id)?.remove();
                    document.getElementById(id)?.remove();
                } finally {
                    host?.remove();
                }
            }
            const result = mermaidCache.get(key);
            node.dataset.rendered = '1';
            if (result.svg) {
                node.innerHTML = result.svg;
            } else {
                const pre = document.createElement('pre');
                pre.className = 'mermaid-error';
                pre.textContent = t('mermaidError', { error: result.error }) + '\n\n' + source;
                node.replaceChildren(pre);
            }
        }
        if (syncedLine !== null && gen === generation) scrollToLine(syncedLine);
    }

    // ---------- images ----------
    article.addEventListener('error', (e) => {
        const img = e.target;
        if (!(img instanceof HTMLImageElement)) return;
        img.classList.add('broken');
        img.title = t('imageNotFound', { src: decodeURIComponent(img.src) });
        if (!img.alt) img.alt = img.getAttribute('src') ?? 'image';
    }, true);

    // Tells the app which local files this page shows, so replacing one on disk refreshes the preview.
    function reportLocalFiles() {
        const urls = new Set();
        document.querySelectorAll('img[src], video[src], source[src], video[poster]').forEach((el) => {
            for (const attr of ['src', 'poster']) {
                const value = el.getAttribute(attr);
                if (!value) continue;
                const url = new URL(value, location.href);
                if (url.origin === location.origin && !url.pathname.startsWith('/__app/')) {
                    url.search = '';
                    urls.add(url.href);
                }
            }
        });
        if (urls.size) post({ type: 'localFiles', urls: [...urls] });
    }

    function refreshImages() {
        const stamp = Date.now();
        document.querySelectorAll('img, video, source').forEach((el) => {
            const src = el.getAttribute('src');
            if (!src || src.startsWith('data:') || src.startsWith(APP)) return;
            const url = new URL(src, location.href);
            if (url.origin !== location.origin) return;
            url.searchParams.set('mv', stamp);
            el.classList.remove('broken');
            el.src = url.href;
        });
    }

    // ---------- links: the app decides (browser, other document, Explorer) ----------
    function onLinkClick(e) {
        if (e.button !== 0 && e.button !== 1) return;
        const a = e.target.closest?.('a[href]');
        const img = e.target instanceof HTMLImageElement ? e.target : null;
        if (!a) {
            if (img && e.button === 0 && !img.classList.contains('broken')) {
                lightbox.querySelector('img').src = img.currentSrc || img.src;
                lightbox.hidden = false;
            }
            return;
        }
        const href = a.getAttribute('href');
        if (href.startsWith('#')) return; // same-page anchor
        e.preventDefault();
        post({ type: 'link', url: a.href, newWindow: e.ctrlKey || e.shiftKey || e.button === 1 });
    }
    article.addEventListener('click', onLinkClick);
    article.addEventListener('auxclick', onLinkClick);
    lightbox.addEventListener('click', () => { lightbox.hidden = true; });
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') lightbox.hidden = true; });

    // ---------- scroll sync (editor -> preview) ----------
    function scrollToLine(line) {
        syncedLine = line;
        if (line <= 0) {
            window.scrollTo(0, 0);
            return;
        }
        let before = null;
        let after = null;
        for (const el of article.querySelectorAll(':scope > [data-sourcepos], li[data-sourcepos], tr[data-sourcepos]')) {
            if (!el.offsetParent && el.tagName !== 'TR') continue;
            const l = lineOf(el);
            if (l <= line) before = el;
            else {
                after = el;
                break;
            }
        }
        if (!before) return;
        const top = (el) => el.getBoundingClientRect().top + window.scrollY;
        const lb = lineOf(before);
        let y = top(before);
        if (after) {
            const la = lineOf(after);
            const frac = Math.min(1, Math.max(0, (line - lb) / Math.max(1, la - lb)));
            y += (top(after) - y) * frac;
        }
        window.scrollTo(0, Math.max(0, y - 16));
    }

    // Double-click jumps the editor to that block.
    article.addEventListener('dblclick', (e) => {
        const owner = e.target.closest?.('[data-sourcepos]');
        if (owner) post({ type: 'reveal', line: lineOf(owner) });
    });

    // ---------- print: always print in light theme ----------
    let themeBeforePrint = null;
    async function printDocument() {
        lightbox.hidden = true;
        flushNear();
        if (theme === 'dark') {
            themeBeforePrint = theme;
            await applyTheme('light');
        }
        window.print();
    }
    window.addEventListener('afterprint', () => {
        if (themeBeforePrint) {
            const t = themeBeforePrint;
            themeBeforePrint = null;
            applyTheme(t);
        }
    });

    // ---------- find in page (Ctrl+F; the editor has its own search panel) ----------
    const findBar = document.getElementById('find-bar');
    const findInput = document.getElementById('find-input');
    const findCount = document.getElementById('find-count');
    // Math is left out: it reads as LaTeX until rendered (lazily, in a large document) and as symbols after.
    const FIND_SKIP = 'style, script, [data-math-style], .math-display, .code-copy-btn, .code-lang';
    const MAX_MATCHES = 10000;
    const FIND_TOP = 60; // the find bar covers the top of the window
    let matches = [];
    let current = -1;

    // Every match of the query as a Range: case-insensitive, with any run of spaces matching any whitespace.
    // The text is joined across elements, so a match can span formatting such as **bold**.
    function findRanges(query) {
        const words = query.trim();
        if (!words) return [];
        const escape = (w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const pattern = new RegExp(words.split(/\s+/).map(escape).join('\\s+'), 'giu');
        const walker = document.createTreeWalker(article, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
            acceptNode: (n) => {
                if (n.nodeType === Node.TEXT_NODE) return NodeFilter.FILTER_ACCEPT;
                // Diagram <text> can't show a highlight, so it is left out rather than counted invisibly.
                if (n.matches(FIND_SKIP) || n instanceof SVGTextContentElement) return NodeFilter.FILTER_REJECT;
                return NodeFilter.FILTER_SKIP;
            },
        });
        const nodes = [];
        const starts = [];
        let text = '';
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
            nodes.push(n);
            starts.push(text.length);
            text += n.data;
        }
        const ranges = [];
        let a = 0;
        for (const m of text.matchAll(pattern)) {
            const from = m.index;
            const to = from + m[0].length;
            while (a + 1 < nodes.length && starts[a + 1] <= from) a++;
            let b = a;
            while (b + 1 < nodes.length && starts[b + 1] < to) b++;
            const range = new Range();
            range.setStart(nodes[a], from - starts[a]);
            range.setEnd(nodes[b], to - starts[b]);
            ranges.push(range);
            if (ranges.length >= MAX_MATCHES) break;
        }
        return ranges;
    }

    function paintMatches() {
        if (!CSS.highlights) return;
        CSS.highlights.set('mv-find', new Highlight(...matches));
        const now = new Highlight(...(matches[current] ? [matches[current]] : []));
        now.priority = 1;
        CSS.highlights.set('mv-find-current', now);
    }

    function showCount() {
        const query = findInput.value.trim();
        const none = query !== '' && !matches.length;
        findCount.textContent = none ? t('findNone')
            : matches.length ? t('findCount', { current: current + 1, total: matches.length }) : '';
        findBar.classList.toggle('no-match', none);
    }

    function revealMatch() {
        const range = matches[current];
        if (!range) return;
        const el = range.startContainer.parentElement;
        for (let d = el.closest('details:not([open])'); d; d = d.closest('details:not([open])')) d.open = true;
        syncedLine = null;
        const visible = (r) => r.height > 0 && r.top >= FIND_TOP && r.bottom <= window.innerHeight - 20;
        if (visible(range.getBoundingClientRect())) return;
        el.scrollIntoView({ block: 'center', inline: 'nearest' });
        const rect = range.getBoundingClientRect();
        if (!visible(rect)) window.scrollBy(0, rect.top - window.innerHeight / 2);
    }

    // keepPlace: re-run after the content changed, without moving the view.
    function runFind(keepPlace = false) {
        matches = findRanges(findInput.value);
        if (keepPlace) {
            current = Math.min(current, matches.length - 1);
            if (current < 0 && matches.length) current = 0;
        } else {
            // Start from the first match at or below the top of the window, as browsers do.
            let lo = 0;
            let hi = matches.length;
            while (lo < hi) {
                const mid = (lo + hi) >> 1;
                if (matches[mid].getBoundingClientRect().bottom < FIND_TOP) lo = mid + 1;
                else hi = mid;
            }
            current = matches.length ? lo % matches.length : -1;
        }
        paintMatches();
        showCount();
        if (!keepPlace) revealMatch();
    }

    function stepFind(delta) {
        if (!matches.length) return;
        current = (current + delta + matches.length) % matches.length;
        paintMatches();
        showCount();
        revealMatch();
    }

    function openFind() {
        const selected = window.getSelection().toString().trim();
        if (selected && selected.length <= 200 && !selected.includes('\n')) findInput.value = selected;
        findBar.hidden = false;
        findInput.focus();
        findInput.select();
        if (findInput.value.trim()) runFind();
    }

    function closeFind() {
        if (findBar.hidden) return;
        findBar.hidden = true;
        // Leave the current match selected, so Ctrl+C copies it.
        const range = matches[current];
        if (range) {
            const selection = window.getSelection();
            selection.removeAllRanges();
            selection.addRange(range);
        }
        matches = [];
        current = -1;
        paintMatches();
    }

    function refreshFind() {
        if (!findBar.hidden && findInput.value.trim()) runFind(true);
    }

    // Large documents highlight code as it scrolls near; re-find once a burst of that settles.
    let refreshTimer = 0;
    function refreshFindSoon() {
        if (findBar.hidden) return;
        clearTimeout(refreshTimer);
        refreshTimer = setTimeout(refreshFind, 150);
    }

    findInput.addEventListener('input', () => runFind());
    findInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
            e.preventDefault();
            stepFind(e.shiftKey ? -1 : 1);
        } else if (e.key === 'Escape') {
            e.stopPropagation();
            closeFind();
        }
    });
    document.getElementById('find-prev').addEventListener('click', () => stepFind(-1));
    document.getElementById('find-next').addEventListener('click', () => stepFind(1));
    document.getElementById('find-close').addEventListener('click', closeFind);
    document.addEventListener('keydown', (e) => {
        const plain = !e.altKey && !e.metaKey;
        if (plain && e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'f') {
            e.preventDefault();
            openFind();
        } else if (plain && !e.ctrlKey && e.key === 'F3') {
            e.preventDefault();
            if (findBar.hidden || !matches.length) openFind();
            else stepFind(e.shiftKey ? -1 : 1);
        } else if (e.key === 'Escape') {
            closeFind();
        }
    });

    // ---------- shell messages ----------
    window.addEventListener('message', (e) => {
        if (e.origin !== SHELL_ORIGIN) return;
        const m = e.data;
        switch (m.type) {
            case 'update':
                article.innerHTML = m.html;
                updateLarge(); // before any layout of the new content
                enhance().then(refreshFind); // again once math and diagrams are in
                refreshFind();
                break;
            case 'find': openFind(); break;
            case 'theme': applyTheme(m.theme); break;
            case 'toc': setToc(m.visible); break;
            case 'zoom': applyZoom(m.zoom); break;
            case 'scrollToLine': scrollToLine(m.line); break;
            case 'unsync': syncedLine = null; break;
            case 'refreshImages': refreshImages(); break;
            case 'print': printDocument(); break;
            case 'strings':
                document.documentElement.lang = m.lang;
                applyStrings(m.strings);
                break;
            case 'scrollToFragment': {
                const target = document.getElementById(m.id);
                if (target) target.scrollIntoView({ block: 'start' });
                break;
            }
        }
    });

    window.addEventListener('scroll', () => requestAnimationFrame(updateActiveHeading), { passive: true });
    // Scrolling the preview by hand stops it from snapping back to the editor's position on the next edit.
    window.addEventListener('wheel', () => { syncedLine = null; }, { passive: true });
    window.addEventListener('keydown', (e) => {
        if (['PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown', ' '].includes(e.key)) syncedLine = null;
    });

    applyStrings(strings);
    setToc(config.toc);
    applyZoom(config.zoom);
    enhance().then(() => { document.body.dataset.ready = '1'; });
    if (location.hash) {
        const target = document.getElementById(decodeURIComponent(location.hash.slice(1)));
        if (target) target.scrollIntoView({ block: 'start' });
    }
    post({ type: 'ready' });
})();
