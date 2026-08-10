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
  vivi: Object.freeze({
    cover: () => import('../../assets/pet/vivi/cover.webp').then(assetUrl),
    atlas: () => import('../../assets/pet/vivi/spritesheet.webp').then(assetUrl),
    walkAtlas: () => import('../../assets/pet/vivi/walk-spritesheet.webp').then(assetUrl),
    dragAtlas: () => import('../../assets/pet/vivi/drag-sit-type.webp').then(assetUrl),
    idleSpecial: () => import('../../assets/pet/vivi/idle-special.webp').then(assetUrl),
  }),
});

export const PET_REGISTRY = Object.freeze(Object.fromEntries(
  manifest.map((pet) => [
    pet.id,
    Object.freeze({
      ...pet,
      cover: PET_LOADERS[pet.id].cover,
      atlas: PET_LOADERS[pet.id].atlas,
      walkAtlas: PET_LOADERS[pet.id].walkAtlas,
      dragAtlas: PET_LOADERS[pet.id].dragAtlas,
      idleSpecial: PET_LOADERS[pet.id].idleSpecial,
    }),
  ]),
));

export function normalizePetId(id) {
  return typeof id === 'string' && Object.hasOwn(PET_REGISTRY, id)
    ? id
    : DEFAULT_PET_ID;
}

export function resolvePet(id) {
  return PET_REGISTRY[normalizePetId(id)];
}
