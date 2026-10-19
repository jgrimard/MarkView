// CodeMirror 6 editor for MarkView, bundled to ui/editor.bundle.js as the global `MVEditor`.
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection,
         dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars } from '@codemirror/view';
import { EditorState, Compartment } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { syntaxHighlighting, HighlightStyle, bracketMatching, indentOnInput } from '@codemirror/language';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { tags } from '@lezer/highlight';

const baseTheme = EditorView.theme({
    '&': { height: '100%', fontSize: '14px' },
    '.cm-scroller': {
        fontFamily: "'Cascadia Code', 'Cascadia Mono', Consolas, 'Courier New', monospace",
        lineHeight: '22px',
    },
    '.cm-content': { padding: '12px 0' },
    '.cm-gutters': { border: 'none' },
    '&.cm-focused': { outline: 'none' },
});

const lightTheme = EditorView.theme({
    '&': { backgroundColor: '#ffffff', color: '#1f2328' },
    '.cm-gutters': { backgroundColor: '#ffffff', color: '#8c959f' },
    // Translucent: the selection is drawn beneath the lines, so an opaque colour would hide it.
    '.cm-activeLine': { backgroundColor: 'rgba(175, 184, 193, 0.15)' },
    '.cm-activeLineGutter': { backgroundColor: '#f6f8fa', color: '#1f2328' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: '#b6d6fd !important' },
    '.cm-cursor': { borderLeftColor: '#1f2328' },
}, { dark: false });

const darkTheme = EditorView.theme({
    '&': { backgroundColor: '#0d1117', color: '#e6edf3' },
    '.cm-gutters': { backgroundColor: '#0d1117', color: '#6e7681' },
    '.cm-activeLine': { backgroundColor: 'rgba(110, 118, 129, 0.1)' },
    '.cm-activeLineGutter': { backgroundColor: '#161b22', color: '#e6edf3' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: '#264f78 !important' },
    '.cm-cursor': { borderLeftColor: '#e6edf3' },
}, { dark: true });

// Markdown colours in the spirit of VS Code's Markdown highlighting (no underlines).
function markdownStyle(c) {
    return HighlightStyle.define([
        { tag: tags.heading, color: c.heading, fontWeight: '600' },
        { tag: tags.strong, fontWeight: '700' },
        { tag: tags.emphasis, fontStyle: 'italic' },
        { tag: tags.strikethrough, textDecoration: 'line-through' },
        { tag: tags.link, color: c.link },
        { tag: tags.url, color: c.url },
        { tag: tags.monospace, color: c.code },
        { tag: tags.quote, color: c.quote },
        { tag: [tags.processingInstruction, tags.punctuation, tags.contentSeparator], color: c.mark },
        { tag: [tags.meta, tags.comment], color: c.quote },
        { tag: [tags.keyword, tags.tagName], color: c.keyword },
        { tag: [tags.string, tags.attributeValue], color: c.url },
        { tag: tags.attributeName, color: c.link },
    ]);
}

const markdownLight = markdownStyle({
    heading: '#0550ae', link: '#0969da', url: '#953800', code: '#cf222e',
    quote: '#57606a', mark: '#8250df', keyword: '#cf222e',
});
const markdownDark = markdownStyle({
    heading: '#79c0ff', link: '#58a6ff', url: '#ffa657', code: '#ff7b72',
    quote: '#8b949e', mark: '#d2a8ff', keyword: '#ff7b72',
});

function themeExtensions(theme) {
    return theme === 'dark'
        ? [darkTheme, syntaxHighlighting(markdownDark)]
        : [lightTheme, syntaxHighlighting(markdownLight)];
}

/**
 * Creates an editor.
 * options: { text, theme, phrases, onChange(text), onScroll(line), onPasteImage(file) }
 * phrases translates CodeMirror's own UI text (the search panel), keyed by the English phrase.
 */
function create(parent, options) {
    const themeSlot = new Compartment();
    const phraseSlot = new Compartment();
    let suppress = false;

    const view = new EditorView({
        parent,
        state: EditorState.create({
            doc: options.text ?? '',
            extensions: [
                lineNumbers(),
                highlightActiveLineGutter(),
                highlightSpecialChars(),
                history(),
                drawSelection(),
                dropCursor(),
                EditorState.allowMultipleSelections.of(true),
                indentOnInput(),
                bracketMatching(),
                rectangularSelection(),
                crosshairCursor(),
                highlightActiveLine(),
                highlightSelectionMatches(),
                keymap.of([...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab]),
                markdown({ base: markdownLanguage }),
                EditorView.lineWrapping,
                baseTheme,
                themeSlot.of(themeExtensions(options.theme)),
                phraseSlot.of(EditorState.phrases.of(options.phrases ?? {})),
                EditorView.updateListener.of((u) => {
                    if (u.docChanged && !suppress) options.onChange?.(u.state.doc.toString());
                }),
                EditorView.domEventHandlers({
                    paste(event) {
                        const item = [...(event.clipboardData?.items ?? [])]
                            .find((i) => i.kind === 'file' && i.type.startsWith('image/'));
                        if (!item) return false;
                        event.preventDefault();
                        options.onPasteImage?.(item.getAsFile());
                        return true;
                    },
                }),
            ],
        }),
    });

    view.scrollDOM.addEventListener('scroll', () => {
        options.onScroll?.(topLine());
    }, { passive: true });

    // 0-based fractional line at the top of the viewport (matches the preview's source lines).
    function topLine() {
        const top = view.scrollDOM.scrollTop;
        const block = view.lineBlockAtHeight(top);
        const line = view.state.doc.lineAt(block.from).number - 1;
        const frac = block.height > 0 ? Math.min(1, Math.max(0, (top - block.top) / block.height)) : 0;
        return line + frac;
    }

    return {
        view,
        getText: () => view.state.doc.toString(),
        setText(text) {
            if (text === view.state.doc.toString()) return;
            const scroll = view.scrollDOM.scrollTop;
            const head = Math.min(view.state.selection.main.head, text.length);
            suppress = true;
            try {
                view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, selection: { anchor: head } });
            } finally {
                suppress = false;
            }
            view.scrollDOM.scrollTop = scroll;
        },
        insert(text) {
            const { from, to } = view.state.selection.main;
            view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length } });
            view.focus();
        },
        replaceLine(line, text) {
            if (line + 1 > view.state.doc.lines) return;
            const l = view.state.doc.line(line + 1);
            view.dispatch({ changes: { from: l.from, to: l.to, insert: text } });
        },
        revealLine(line) {
            const l = view.state.doc.line(Math.min(view.state.doc.lines, line + 1));
            view.dispatch({ selection: { anchor: l.from }, effects: EditorView.scrollIntoView(l.from, { y: 'center' }) });
            view.focus();
        },
        moveCursorToPoint(x, y) {
            const pos = view.posAtCoords({ x, y });
            if (pos !== null) view.dispatch({ selection: { anchor: pos } });
        },
        setTheme(theme) {
            view.dispatch({ effects: themeSlot.reconfigure(themeExtensions(theme)) });
        },
        setPhrases(phrases) {
            view.dispatch({ effects: phraseSlot.reconfigure(EditorState.phrases.of(phrases)) });
        },
        focus: () => view.focus(),
        topLine,
        destroy: () => view.destroy(),
    };
}

window.MVEditor = { create };
