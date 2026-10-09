/** Contract errors deliberately exclude submitted values and secret material. */
export class ContractError extends Error {
  constructor(code) {
    super(code);
    this.name = 'ContractError';
    this.code = code;
  }
}

export function requireCondition(condition, code) {
  if (!condition) throw new ContractError(code);
}
