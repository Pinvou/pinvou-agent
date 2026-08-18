const PROTOCOL_VERSION = 'v0.9';
const NAMESPACE = 'projection';
const SURFACE_ID = 'projection/runtime-overview';
const CATALOG_ID = 'urn:pinvou:a2ui:catalog:projection:v1';
const MESSAGE_KEYS = ['createSurface', 'updateComponents', 'updateDataModel', 'deleteSurface'];
const COMPONENT_CATALOG = new Set([
  'PinvouCanvas',
  'PinvouIdentityCard',
  'PinvouInteractionCard',
  'PinvouRuntimeHealth',
]);

export const EMPTY_A2UI_STATE = Object.freeze({
  basisSequence: 0,
  surfaceId: '',
  components: {},
  dataModel: {},
});

function protocolError(message) {
  throw new Error(`invalid_a2ui_projection:${message}`);
}

function messageKind(message) {
  if (!message || typeof message !== 'object' || message.version !== PROTOCOL_VERSION) {
    protocolError('unsupported_version');
  }
  const keys = MESSAGE_KEYS.filter(key => Object.prototype.hasOwnProperty.call(message, key));
  if (keys.length !== 1) protocolError('message_requires_exactly_one_operation');
  return keys[0];
}

function requireSurface(operation) {
  if (!operation || operation.surfaceId !== SURFACE_ID) protocolError('surface_scope_mismatch');
}

function validateComponents(components) {
  if (!Array.isArray(components)) protocolError('components_must_be_an_array');
  const ids = new Set();
  components.forEach(component => {
    if (!component || typeof component.id !== 'string' || !component.id) {
      protocolError('component_id_required');
    }
    if (ids.has(component.id)) protocolError('duplicate_component_id');
    if (!COMPONENT_CATALOG.has(component.component)) protocolError('component_not_in_catalog');
    ids.add(component.id);
  });
  components.forEach(component => {
    const children = Array.isArray(component.children) ? component.children : [];
    children.forEach(childId => {
      if (!ids.has(childId)) protocolError('unknown_child_component');
    });
  });
  if (!ids.has('root')) protocolError('root_component_required');
}

export function applyA2uiProjection(current, projection) {
  if (!projection || projection.namespace !== NAMESPACE || !Array.isArray(projection.messages)) {
    protocolError('projection_envelope_invalid');
  }
  const basisSequence = Number(projection.basisSequence);
  if (!Number.isSafeInteger(basisSequence) || basisSequence < 0) {
    protocolError('basis_sequence_invalid');
  }
  if (basisSequence < Number(current && current.basisSequence || 0)) return current;

  let next = {
    basisSequence,
    surfaceId: current && current.surfaceId || '',
    components: { ...(current && current.components || {}) },
    dataModel: { ...(current && current.dataModel || {}) },
  };

  projection.messages.forEach(message => {
    const kind = messageKind(message);
    const operation = message[kind];
    requireSurface(operation);
    if (kind === 'createSurface') {
      if (operation.catalogId !== CATALOG_ID || operation.sendDataModel !== false) {
        protocolError('catalog_or_data_sync_not_allowed');
      }
      next = { ...next, surfaceId: SURFACE_ID, components: {}, dataModel: {} };
      return;
    }
    if (kind === 'deleteSurface') {
      next = { ...next, surfaceId: '', components: {}, dataModel: {} };
      return;
    }
    if (next.surfaceId !== SURFACE_ID) protocolError('surface_not_created');
    if (kind === 'updateComponents') {
      validateComponents(operation.components);
      next.components = Object.fromEntries(operation.components.map(component => [component.id, component]));
      return;
    }
    if (operation.path !== '/' || !operation.value || typeof operation.value !== 'object') {
      protocolError('only_root_data_model_updates_are_allowed');
    }
    next.dataModel = operation.value;
  });
  return next;
}

export function resolveA2uiBinding(value, dataModel) {
  if (!value || typeof value !== 'object' || typeof value.path !== 'string') return value;
  if (value.path === '/') return dataModel;
  return value.path.split('/').slice(1).reduce((current, segment) => {
    if (current == null || typeof current !== 'object') return undefined;
    const key = segment.replace(/~1/g, '/').replace(/~0/g, '~');
    return current[key];
  }, dataModel);
}

export const PINVOU_A2UI_CONTRACT = Object.freeze({
  protocolVersion: PROTOCOL_VERSION,
  namespace: NAMESPACE,
  surfaceId: SURFACE_ID,
  catalogId: CATALOG_ID,
  componentCatalog: COMPONENT_CATALOG,
});
