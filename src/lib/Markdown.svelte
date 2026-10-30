<script lang="ts">
    import { marked } from 'marked';
    import hljs from 'highlight.js';
    import 'highlight.js/styles/xcode.min.css';

    let { content = '' } = $props();
    
    // Marked with highlight.js
    marked.setOptions({
        gfm: true,
        breaks: true,
    });

    // Customize renderer to use highlight.js for code blocks
    const renderer = new marked.Renderer();
    renderer.code = ({ text, lang }) => {
        const validLang = lang && hljs.getLanguage(lang) ? lang : 'plaintext';
        const highlighted = hljs.highlight(text, { language: validLang }).value;
        return `<pre class="my-3 p-3 rounded-xl bg-[#0e0f12] border border-zinc-800/80 overflow-x-auto font-mono text-xs text-emerald-300 leading-relaxed"><code>${highlighted}</code></pre>`;
    };

    renderer.table = (token) => {
        // Render header cells
        const headerHtml = token.header
        .map((cell) => `<th class="p-2.5 font-semibold text-zinc-300 border-b border-zinc-800">${marked.parseInline(cell.text)}</th>`)
        .join('');

        // Render body rows
        const bodyHtml = token.rows
        .map((row) => {
            const rowCells = row
            .map((cell) => `<td class="p-2.5 text-zinc-300 border-b border-zinc-800/40">${marked.parseInline(cell.text)}</td>`)
            .join('');
            return `<tr class="hover:bg-zinc-800/30 transition-colors">${rowCells}</tr>`;
        })
        .join('');

        return `<div class="my-3 overflow-x-auto border border-zinc-800 rounded-xl">
                    <table class="w-full text-xs text-left border-collapse">
                        <thead class="bg-[#13151a] font-mono">
                        <tr>${headerHtml}</tr>
                        </thead>
                        <tbody class="divide-y divide-zinc-800/60 bg-[#181b22]/50">
                        ${bodyHtml}
                        </tbody>
                    </table>
                </div>`;
    };

    let htmlContent = $derived(marked.parse(content, { renderer }));
</script>

<div class="prose prose-invert max-w-none text-xs md:text-sm leading-relaxed space-y-2">
  {@html htmlContent}
</div>