import manifest from './pet-manifest.json';

export const DEFAULT_PET_ID = 'lingling';

function assetUrl(module) {
  return module.default;
}

export const PET_LOADERS = Object.freeze({
  lingling: Object.freeze({
    cover: () => import('../../assets/pet/lingling/cover.webp').then(assetUrl),
    atlas: () => import('../../assets/pet/lingling/spritesheet.webp').then(assetUrl),
  }),
  langlang: Object.freeze({
    cover: () => import('../../assets/pet/langlang/cover.webp').then(assetUrl),
    atlas: () => import('../../assets/pet/langlang/spritesheet.webp').then(assetUrl),
  }),
  'ace-taffy': Object.freeze({
    cover: () => import('../../assets/pet/ace-taffy/cover.webp').then(assetUrl),
    atlas: () => import('../../assets/pet/ace-taffy/spritesheet.webp').then(assetUrl),
  }),
});

export const PET_REGISTRY = Object.freeze(Object.fromEntries(
  manifest.map((pet) => [
    pet.id,
    Object.freeze({
      ...pet,
      cover: PET_LOADERS[pet.id].cover,
      atlas: PET_LOADERS[pet.id].atlas,
    }),
  ]),
));

export function normalizePetId(id) {
  // hasOwnProperty.call 而非 Object.hasOwn：Safari 14(WKWebView)无后者。
  return typeof id === 'string' && Object.prototype.hasOwnProperty.call(PET_REGISTRY, id)
    ? id
    : DEFAULT_PET_ID;
}

export function resolvePet(id) {
  return PET_REGISTRY[normalizePetId(id)];
}
