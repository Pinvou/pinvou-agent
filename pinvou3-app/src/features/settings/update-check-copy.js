// Errors carry fixed backend codes or opaque transport failures. Only the
// unsupported code gets dedicated copy; raw details stay out of the UI.
export function updateErrorCopy(state, text) {
  if (state?.updateError) return text.updateInstallFailed;
  if (state?.updateCheckError === 'UPDATER_CHECK_UNSUPPORTED') return text.updateCheckUnsupported;
  if (state?.updateCheckError && state.updateCheckError !== 'latest') return text.updateCheckFailed;
  return '';
}
