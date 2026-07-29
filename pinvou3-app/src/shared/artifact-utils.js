

// 文件类型图标:图形取自 material-icon-theme(PKief/vscode-material-icon-theme,
// MIT License,经 Iconify API 导出),全彩填充 SVG body 内联于此,零运行时依赖。
// glyph 为 <svg> 内部内容;viewBox 缺省 0 0 24 24,尺寸不同的图标单独标注。
const _ARTIFACT_FMT = {
      pptx: { label: 'PPTX', glyph: '<path fill="#e64a19" d="M6 2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2m7 1.5V9h5.5zM8 11v2h1v6H8v1h4v-1h-1v-2h2a3 3 0 0 0 3-3a3 3 0 0 0-3-3zm5 2a1 1 0 0 1 1 1a1 1 0 0 1-1 1h-2v-2z"/>' },
      docx: { label: 'DOCX', glyph: '<path fill="#01579b" d="M6 2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2m7 1.5V9h5.5zM7 13l1.5 7h2l1.5-3l1.5 3h2l1.5-7h1v-2h-4v2h1l-.9 4.2L13 15h-2l-1.1 2.2L9 13h1v-2H6v2z"/>' },
      xlsx: { label: 'XLSX', glyph: '<path fill="#8bc34a" d="M6 2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2m7 1.5V9h5.5zm4 7.5h-4v2h1l-2 1.67L10 13h1v-2H7v2h1l3 2.5L8 18H7v2h4v-2h-1l2-1.67L14 18h-1v2h4v-2h-1l-3-2.5l3-2.5h1z"/>' },
      pdf: { label: 'PDF', glyph: '<path fill="#ef5350" d="M13 9h5.5L13 3.5zM6 2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2m4.93 10.44c.41.9.93 1.64 1.53 2.15l.41.32c-.87.16-2.07.44-3.34.93l-.11.04l.5-1.04c.45-.87.78-1.66 1.01-2.4m6.48 3.81c.18-.18.27-.41.28-.66c.03-.2-.02-.39-.12-.55c-.29-.47-1.04-.69-2.28-.69l-1.29.07l-.87-.58c-.63-.52-1.2-1.43-1.6-2.56l.04-.14c.33-1.33.64-2.94-.02-3.6a.85.85 0 0 0-.61-.24h-.24c-.37 0-.7.39-.79.77c-.37 1.33-.15 2.06.22 3.27v.01c-.25.88-.57 1.9-1.08 2.93l-.96 1.8l-.89.49c-1.2.75-1.77 1.59-1.88 2.12c-.04.19-.02.36.05.54l.03.05l.48.31l.44.11c.81 0 1.73-.95 2.97-3.07l.18-.07c1.03-.33 2.31-.56 4.03-.75c1.03.51 2.24.74 3 .74c.44 0 .74-.11.91-.3m-.41-.71l.09.11c-.01.1-.04.11-.09.13h-.04l-.19.02c-.46 0-1.17-.19-1.9-.51c.09-.1.13-.1.23-.1c1.4 0 1.8.25 1.9.35M7.83 17c-.65 1.19-1.24 1.85-1.69 2c.05-.38.5-1.04 1.21-1.69zm3.02-6.91c-.23-.9-.24-1.63-.07-2.05l.07-.12l.15.05c.17.24.19.56.09 1.1l-.03.16l-.16.82z"/>' },
      image: { label: 'IMG', viewBox: '0 0 16 16', glyph: '<path fill="#26a69a" d="M8.5 6h4l-4-4zM3.875 1H9.5l4 4v8.6c0 .773-.616 1.4-1.375 1.4h-8.25c-.76 0-1.375-.627-1.375-1.4V2.4c0-.777.612-1.4 1.375-1.4M4 13.6h8V8l-2.625 2.8L8 9.4zm1.25-7.7c-.76 0-1.375.627-1.375 1.4s.616 1.4 1.375 1.4c.76 0 1.375-.627 1.375-1.4S6.009 5.9 5.25 5.9"/>' },
      html: { label: 'HTML', viewBox: '0 0 32 32', glyph: '<path fill="#e65100" d="m4 4l2 22l10 2l10-2l2-22Zm19.72 7H11.28l.29 3h11.86l-.802 9.335L15.99 25l-6.635-1.646L8.93 19h3.02l.19 2l3.86.77l3.84-.77l.29-4H8.84L8 8h16Z"/>' },
      markdown: { label: 'MD', viewBox: '0 0 32 32', glyph: '<path fill="#42a5f5" d="m14 10l-4 3.5L6 10H4v12h4v-6l2 2l2-2v6h4V10zm12 6v-6h-4v6h-4l6 8l6-8z"/>' },
      archive: { label: 'ZIP', glyph: '<path fill="#afb42b" d="M14 17h-2v-2h-2v-2h2v2h2m0-6h-2v2h2v2h-2v-2h-2V9h2V7h-2V5h2v2h2m5-4H5c-1.11 0-2 .89-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2"/>' },
      audio: { label: 'AUDIO', viewBox: '0 0 32 32', glyph: '<path fill="#ef5350" d="M16 2a14 14 0 1 0 14 14A14 14 0 0 0 16 2m6 10h-4v8a4 4 0 1 1-4-4a3.96 3.96 0 0 1 2 .555V8h6Z"/>' },
      video: { label: 'VIDEO', viewBox: '0 0 32 32', glyph: '<path fill="#ff9800" d="m24 6l2 6h-4l-2-6h-3l2 6h-4l-2-6h-3l2 6H8L6 6H5a3 3 0 0 0-3 3v14a3 3 0 0 0 3 3h22a3 3 0 0 0 3-3V6Z"/>' },
      code: { label: 'CODE', viewBox: '0 0 16 16', glyph: '<path fill="#ff7043" d="M2 2a1 1 0 0 0-1 1v10c0 .554.446 1 1 1h12c.554 0 1-.446 1-1V3a1 1 0 0 0-1-1zm0 3h12v8H2zm1 2l2 2l-2 2l1 1l3-3l-3-3zm5 3.5V12h5v-1.5z"/>' },
      data: { label: 'DATA', viewBox: '0 0 960 960', glyph: '<path fill="#f9a825" d="M560-160v-80h120q17 0 28.5-11.5T720-280v-80q0-38 22-69t58-44v-14q-36-13-58-44t-22-69v-80q0-17-11.5-28.5T680-720H560v-80h120q50 0 85 35t35 85v80q0 17 11.5 28.5T840-560h40v160h-40q-17 0-28.5 11.5T800-360v80q0 50-35 85t-85 35zm-280 0q-50 0-85-35t-35-85v-80q0-17-11.5-28.5T120-400H80v-160h40q17 0 28.5-11.5T160-600v-80q0-50 35-85t85-35h120v80H280q-17 0-28.5 11.5T240-680v80q0 38-22 69t-58 44v14q36 13 58 44t22 69v80q0 17 11.5 28.5T280-240h120v80z"/>' },
      other: { label: 'FILE', glyph: '<g fill="none"><path d="M0 0h24v24H0z"/><path fill="#42a5f5" d="M8 16h8v2H8zm0-4h8v2H8zm6-10H6c-1.1 0-2 .9-2 2v16c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8zm4 18H6V4h7v5h5z"/></g>' },
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
      if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz'].indexOf(ext) >= 0) return 'archive';
      if (['mp3', 'wav', 'm4a', 'flac', 'ogg', 'aac'].indexOf(ext) >= 0) return 'audio';
      if (['mp4', 'mov', 'avi', 'mkv', 'webm'].indexOf(ext) >= 0) return 'video';
      if (['js', 'mjs', 'cjs', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'java', 'c', 'h', 'cpp', 'hpp', 'cs', 'sh', 'bat', 'ps1', 'rb', 'php', 'swift', 'kt', 'vue', 'css', 'scss'].indexOf(ext) >= 0) return 'code';
      if (['json', 'xml', 'yml', 'yaml', 'toml', 'sql', 'ini'].indexOf(ext) >= 0) return 'data';
      return 'other';
    };

export { _ARTIFACT_FMT, _artifactKind };
