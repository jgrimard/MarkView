---
title: MarkView test document
---

# MarkView test document

This file exercises every kind of image reference and the main Markdown features.

## Images

### Relative path

![Relative image](img/a.png)

### Parent-relative path

![Parent relative SVG](../shared/b.svg)

### Absolute Windows path

> The two absolute-path examples below point at where this repository lived on the author's machine.
> Edit them to match your clone to test absolute paths.

![Absolute path](C:\Users\jgrimard\Desktop\Source Code\Markdown Viewer\samples\shared\abs.png)

### file:/// URL

![File URL](file:///C:/Users/jgrimard/Desktop/Source%20Code/Markdown%20Viewer/samples/shared/file-url.png)

### Raw HTML with width

<p align="center">
  <img src="img/raw.png" width="200" alt="raw html image">
</p>

### Spaces and Unicode in the file name

![Spaces and unicode](<img/my pic é.png>)

### Animated GIF

![Animated](img/anim.gif)

### Remote images

![Shields badge](https://img.shields.io/badge/remote-image-blue) <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-B7410E?logo=rust&logoColor=white">

### Missing image (should show a red placeholder)

![This one does not exist](img/missing.png)

## Text features

A paragraph with **bold**, *italic*, ~~strikethrough~~, `inline code`, a footnote[^1], an emoji :rocket:,
and a [link to another document](other.md) plus an [external link](https://github.com).

> [!NOTE]
> GitHub-style alert blocks are supported.

> [!WARNING]
> So are warnings.

### Task list

- [x] Render images correctly
- [ ] Toggle me (this edits the source)
- [ ] Me too

### Table

| Feature | MarkLight | MarkView |
|:--------|:---------:|:--------:|
| Local images | No | Yes |
| Live reload | No | Yes |
| Editor | No | Yes |

## Code

```csharp
public static string Greet(string name) => $"Hello, {name}!";
```

```python
def fib(n):
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a
```

```powershell
Get-ChildItem -Recurse -Filter *.md | Select-Object FullName
```

## Mermaid

```mermaid
graph LR
    A[Markdown file] --> B(Markdig)
    B --> C{WebView2}
    C -->|images| D[Local file server]
    C -->|diagrams| E[Mermaid]
```

## Math

Inline math $E = mc^2$ and a display equation:

$$
\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}
$$

[^1]: This is the footnote text.
