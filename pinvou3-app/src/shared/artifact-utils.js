

const _ARTIFACT_FMT = {
      pptx:     { label: 'PPTX', color: '#D24726', glyph: '<path d="M2 3h20"/><path d="M21 3v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V3"/><path d="m7 21 5-5 5 5"/>' },
      docx:     { label: 'DOCX', color: '#2B579A', glyph: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/><path d="M10 9H8"/>' },
      xlsx:     { label: 'XLSX', color: '#217346', glyph: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M3 15h18"/><path d="M12 3v18"/>' },
      pdf:      { label: 'PDF',  color: '#E5352B', glyph: '<path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><path d="M14 2v6h6"/><path d="M9 13h6"/><path d="M9 17h3"/>' },
      image:    { label: 'IMG',  color: '#8E8E93', glyph: '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21"/>' },
      html:     { label: 'HTML', color: '#22A7C2', glyph: '<path d="m18 16 4-4-4-4"/><path d="m6 8-4 4 4 4"/><path d="m14.5 4-5 16"/>' },
      markdown: { label: 'MD',   color: '#6B7280', glyph: '<rect width="20" height="16" x="2" y="4" rx="2"/><path d="M6 16V8l3 3 3-3v8"/><path d="M18 8v6l-2-2"/><path d="m16 12 2 2 2-2"/>' },
      other:    { label: 'FILE', color: '#6B7280', glyph: '<path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>' },
    };
    const _artifactKind = (p) => {
      const ext = (String(p || '').split('.').pop() || '').toLowerCase();
      if (ext === 'pptx' || ext === 'ppt') return 'pptx';
      if (ext === 'docx' || ext === 'doc') return 'docx';
      if (ext === 'xlsx' || ext === 'xls' || ext === 'csv') return 'xlsx';
      if (ext === 'pdf') return 'pdf';
      if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp'].indexOf(ext) >= 0) return 'image';
      if (ext === 'html' || ext === 'htm') return 'html';
      if (ext === 'md' || ext === 'markdown') return 'markdown';
      return 'other';
    };
    // 文件类型图标（白色，置于配色 tile 上）。

export { _ARTIFACT_FMT, _artifactKind };
