import { marked, type Tokens } from 'marked';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/common';

/**
 * Turns model output (untrusted: it can echo attacker-controlled documents or web content) into
 * HTML that is safe to insert with {@html}. Three layers:
 *   1. raw HTML in the text is shown as text, except <br>
 *   2. DOMPurify removes everything that can execute, load remote content, or submit data
 *   3. only highlight.js class names survive, so injected markup cannot borrow this app's
 *      Tailwind classes to draw a fake full-screen "enter your password" overlay
 */

const escapeHtml = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');

marked.use({
  gfm: true,
  breaks: true,
  renderer: {
    code({ text, lang }: Tokens.Code) {
      const language = lang && hljs.getLanguage(lang) ? lang : undefined;
      const html = language
        ? hljs.highlight(text, { language }).value
        : hljs.highlightAuto(text).value;
      return `<pre><code class="hljs${language ? ` language-${language}` : ''}">${html}</code></pre>`;
    },
    // Layer 1: raw HTML becomes visible text (a bare <br> is common in model-written tables)
    html({ text }: Tokens.HTML | Tokens.Tag) {
      return /^<br\s*\/?>$/i.test(text.trim()) ? '<br>' : escapeHtml(text);
    }
  }
});

const ALLOWED_CLASS = /^(?:hljs(?:-[a-z0-9_-]+)?|language-[a-z0-9+#_-]+)$/i;

const purify = DOMPurify;

// Layer 3, plus safe links
purify.addHook('afterSanitizeAttributes', (node) => {
  const cls = (node.getAttribute?.('class') ?? '').split(/\s+/).filter((c) => ALLOWED_CLASS.test(c));
  if (cls.length) node.setAttribute('class', cls.join(' '));
  else node.removeAttribute?.('class');

  if (node.tagName === 'A') {
    node.setAttribute('rel', 'noopener noreferrer nofollow');
    // Show where a link really goes, since the visible text can say anything
    const href = node.getAttribute('href');
    if (href) node.setAttribute('title', href);
  }
});

// Layer 2
const SANITIZE = {
  USE_PROFILES: { html: true },
  FORBID_TAGS: [
    'style', 'script', 'noscript', 'template', 'link', 'meta', 'base',
    'form', 'input', 'button', 'textarea', 'select', 'option', 'label',
    'img', 'picture', 'source', 'video', 'audio', 'track', 'canvas',
    'iframe', 'frame', 'frameset', 'object', 'embed', 'applet',
    'svg', 'math', 'dialog'
  ],
  // `id`/`name` also block DOM clobbering; `style` blocks CSS-based tricks
  FORBID_ATTR: ['style', 'target', 'src', 'srcset', 'action', 'formaction', 'ping', 'id', 'name', 'autofocus'],
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:)/i,
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: false,
  SANITIZE_DOM: true,
  KEEP_CONTENT: true
};

export function renderMarkdown(source: string): string {
  return purify.sanitize(marked.parse(source, { async: false }) as string, SANITIZE) as unknown as string;
}
