import assert from 'node:assert/strict';
import test from 'node:test';

import {
  ARTIFACT_BROWSER_MOTION_MS,
  artifactBrowserLaunchTransform,
} from '../src/features/artifacts/artifact-browser-motion.js';

test('artifact browser launch transform maps the source card to the viewer shell', () => {
  const launch = artifactBrowserLaunchTransform(
    { left: 100, top: 200, width: 200, height: 100 },
    { left: 0, top: 0, width: 1000, height: 500 },
  );

  assert.deepEqual(launch, {
    translateX: -300,
    translateY: 0,
    scaleX: 0.2,
    scaleY: 0.2,
    css: 'translate3d(-300px, 0px, 0) scale(0.2, 0.2)',
  });
  assert.equal(ARTIFACT_BROWSER_MOTION_MS, 460);
});

test('artifact browser launch transform rejects incomplete geometry', () => {
  assert.equal(artifactBrowserLaunchTransform(null, {}), null);
  assert.equal(artifactBrowserLaunchTransform(
    { left: 0, top: 0, width: 0, height: 10 },
    { left: 0, top: 0, width: 100, height: 100 },
  ), null);
  assert.equal(artifactBrowserLaunchTransform(
    { left: Number.NaN, top: 0, width: 10, height: 10 },
    { left: 0, top: 0, width: 100, height: 100 },
  ), null);
});
