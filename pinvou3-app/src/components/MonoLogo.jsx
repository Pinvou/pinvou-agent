// Monochrome brand mark: uses the image's alpha channel as a CSS mask and
// colors it with bg-current (black in light theme, white in dark). Shared by
// CodexLogo and the code-mode Pinvou mark (PinvouMonoLogo in AcpAgentLogo);
// the blue work-mode brand image stays in PinvouLogo and never goes here.
// With a title, expose as an image (role="img" + aria-label; a masked image
// cannot embed <title>); without one, treat as decorative and hide from
// assistive tech. Two branches keep role and aria-label statically analyzable,
// avoiding a conditional role that would trip useAriaPropsSupportedByRole.
export function MonoLogo({ url, className = 'h-4 w-4', title }) {
  const maskStyle = {
    WebkitMaskImage: `url("${url}")`,
    maskImage: `url("${url}")`,
    WebkitMaskPosition: 'center',
    maskPosition: 'center',
    WebkitMaskRepeat: 'no-repeat',
    maskRepeat: 'no-repeat',
    WebkitMaskSize: 'contain',
    maskSize: 'contain',
  };
  const cls = `inline-block shrink-0 bg-current ${className}`;
  if (title) {
    return <span role="img" aria-label={title} className={cls} style={maskStyle} />;
  }
  return <span aria-hidden="true" className={cls} style={maskStyle} />;
}
