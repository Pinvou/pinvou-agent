// Canonical Pinvou scene registry: the single source of truth for the routed
// scene keys, their `lane:key` tags, and the display order. These keys were
// previously re-spelled across pinvou-mode-state.js (subtab whitelist),
// work-scene-routes.js / visual-poster-scene.js (message-meta builders),
// scene-capabilities.js (capability preflight) and ChatView.jsx (scene cards
// and the projected-scene display); adding a scene now means adding one entry
// to PINVOU_SCENES and deriving the rest.
//
// Pure data, no imports: the scene logic vm tests (pinvou_mode_state /
// personal_workbench_scene_logic) concatenate this file with the consuming
// modules, so it must stay dependency-free.
const WORK_SCENE_LANE = 'work';
const DESIGN_SCENE_LANE = 'design';

export const PERSONAL_WORKBENCH_SCENE_KEY = 'personal-workbench';
export const DOCUMENT_WRITING_SCENE_KEY = 'document-writing';
export const POSTER_SCENE_KEY = 'poster';
export const DATA_VISUALIZATION_SCENE_KEY = 'data-visualization';
export const PPT_DESIGN_SCENE_KEY = 'ppt';

// Routed scenes in canonical display order. `lane` prefixes the `pinvouScene`
// tag stamped into message meta and keyed by scene-capabilities.js.
const PINVOU_SCENES = Object.freeze([
  Object.freeze({ key: PERSONAL_WORKBENCH_SCENE_KEY, lane: WORK_SCENE_LANE }),
  Object.freeze({ key: DOCUMENT_WRITING_SCENE_KEY, lane: WORK_SCENE_LANE }),
  Object.freeze({ key: POSTER_SCENE_KEY, lane: DESIGN_SCENE_LANE }),
  Object.freeze({ key: DATA_VISUALIZATION_SCENE_KEY, lane: DESIGN_SCENE_LANE }),
  Object.freeze({ key: PPT_DESIGN_SCENE_KEY, lane: DESIGN_SCENE_LANE }),
]);

export const PINVOU_SCENE_KEYS = Object.freeze(PINVOU_SCENES.map((scene) => scene.key));

// Full `lane:key` scene tag for a routed scene key ('' for unknown keys).
export function pinvouSceneTag(key) {
  const scene = PINVOU_SCENES.find((entry) => entry.key === key);
  return scene ? `${scene.lane}:${scene.key}` : '';
}
