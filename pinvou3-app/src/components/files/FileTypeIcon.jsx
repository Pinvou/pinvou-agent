import { _ARTIFACT_FMT, _artifactKind } from '../../shared/artifact-utils.js';

export function FileTypeIcon({ name, kind, className = '' }) {
  const resolvedKind = kind || _artifactKind(name);
  const format = _ARTIFACT_FMT[resolvedKind] || _ARTIFACT_FMT.other;
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox={format.viewBox || '0 0 24 24'}
      className={className}
      dangerouslySetInnerHTML={{ __html: format.glyph }}
    />
  );
}
