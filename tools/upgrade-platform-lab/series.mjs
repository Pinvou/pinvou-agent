import { runSeries, inspectSeries, readSeriesPlan } from './src/series.mjs';
import { requireThat } from './src/owned-files.mjs';

try {
  const [command, ...args] = process.argv.slice(2);
  if (command === 'run') {
    requireThat(args.length === 2);
    const plan = await readSeriesPlan(args[1]);
    const controller = new AbortController();
    // Stop admission of the next attempt. The active attempt still joins every
    // writer and finishes its owned IO; a signal is never successful close proof.
    const stop = () => controller.abort();
    process.on('SIGINT', stop); process.on('SIGTERM', stop);
    try {
      const result = await runSeries(args[0], plan, { signal: controller.signal });
      console.log(JSON.stringify(result));
      if (result.report.status !== 'complete') process.exitCode = 1;
    } finally { process.off('SIGINT', stop); process.off('SIGTERM', stop); }
  } else if (command === 'inspect') {
    requireThat(args.length === 1);
    const result = await inspectSeries(args[0]);
    console.log(JSON.stringify(result));
    if (!result.published || result.report.status !== 'complete') process.exitCode = 1;
  } else throw new Error('usage');
} catch (error) {
  console.error(JSON.stringify({ error: error?.code?.startsWith('lab_') ? error.code : 'lab_series_failed' }));
  process.exitCode = 1;
}
