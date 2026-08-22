import './state.js';
globalThis.insertFormat = function insertFormat(type) {
    if (!editorInstance) return;
    editorInstance.focus();

    const doc = editorInstance.getDoc();
    const selection = doc.getSelection();
    const cursor = doc.getCursor();

    switch (type) {
        case 'bold':
            doc.replaceSelection('**' + (selection || 'text') + '**');
            if (!selection) {
                const start = doc.getCursor('start');
                doc.setSelection({ line: start.line, ch: start.ch - 6 }, { line: start.line, ch: start.ch - 2 });
            }
            break;
        case 'italic':
            doc.replaceSelection('*' + (selection || 'text') + '*');
            if (!selection) {
                const start = doc.getCursor('start');
                doc.setSelection({ line: start.line, ch: start.ch - 5 }, { line: start.line, ch: start.ch - 1 });
            }
            break;
        case 'header':
            const lineNo = cursor.line;
            const lineText = doc.getLine(lineNo);
            const headerMatch = lineText.match(/^(#{1,4})\s(.*)$/);
            
            if (headerMatch) {
                const currentLevel = headerMatch[1].length;
                const content = headerMatch[2];
                if (currentLevel < 4) {
                    const newPrefix = '#'.repeat(currentLevel + 1) + ' ';
                    doc.replaceRange(newPrefix + content, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
                } else {
                    doc.replaceRange(content, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
                }
            } else {
                doc.replaceRange('# ' + lineText, { line: lineNo, ch: 0 }, { line: lineNo, ch: lineText.length });
            }
            break;
        case 'link':
            const linkUrl = selection.match(/^https?:\/\//) ? selection : 'https://example.com';
            const linkText = selection.match(/^https?:\/\//) ? 'Link' : (selection || 'Link text');
            doc.replaceSelection(`[${linkText}](${linkUrl})`);
            break;
        case 'image':
            const imgUrl = selection.match(/^https?:\/\//) ? selection : 'https://example.com/image.png';
            const imgAlt = selection.match(/^https?:\/\//) ? 'Alt' : (selection || 'Alt text');
            doc.replaceSelection(`![${imgAlt}](${imgUrl})`);
            break;
        case 'code':
            doc.replaceSelection('```\n' + (selection || 'code') + '\n```');
            break;
        case 'ul':
            transformLines(line => line.startsWith('- ') ? line.substring(2) : '- ' + line);
            break;
        case 'ol':
            transformLines((line, i) => {
                const match = line.match(/^(\d+)\.\s(.*)$/);
                return match ? match[2] : (i + 1) + '. ' + line;
            });
            break;
        case 'task':
            transformLines(line => {
                const match = line.match(/^- \[[ xX]\] (.*)$/);
                return match ? match[1] : '- [ ] ' + line;
            });
            break;
        case 'table':
            doc.replaceSelection(
                '\n| Column 1 | Column 2 |\n' +
                '| -------- | -------- |\n' +
                '| Cell 1   | Cell 2   |\n'
            );
            break;
    }

    function transformLines(fn) {
        editorInstance.operation(() => {
            const start = doc.getCursor('start').line;
            const end = doc.getCursor('end').line;
            for (let i = start; i <= end; i++) {
                const line = doc.getLine(i);
                doc.replaceRange(fn(line, i - start), { line: i, ch: 0 }, { line: i, ch: line.length });
            }
        });
    }
}
