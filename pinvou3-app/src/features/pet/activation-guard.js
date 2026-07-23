export const PET_ACTIVATION_GUARD_MS = 220;

export function createPetActivationGuard({
  now = () => performance.now(),
  durationMs = PET_ACTIVATION_GUARD_MS,
} = {}) {
  let armedUntil = 0;

  const arm = () => {
    armedUntil = now() + durationMs;
  };

  const handleClick = (event) => {
    if (armedUntil === 0 || now() > armedUntil) {
      armedUntil = 0;
      return false;
    }
    // 一次桌宠唤醒只消费一次激活 click，避免短时间内的下一次真实点击也被吞。
    armedUntil = 0;
    event.preventDefault();
    event.stopPropagation();
    if (typeof event.stopImmediatePropagation === 'function') {
      event.stopImmediatePropagation();
    }
    return true;
  };

  return { arm, handleClick };
}
