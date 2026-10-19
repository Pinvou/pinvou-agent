import { workload } from './src/workload.mjs';
import { runExperiment, summarize } from './src/run.mjs';
import { requireThat } from './src/owned-files.mjs';
import { readFile } from 'node:fs/promises';

try {
  const [command, ...args] = process.argv.slice(2);
  if (command === 'run') {
    requireThat(args.length === 3 || args.length === 4);
    const [parent, profile, shape, contextFile] = args;
    const context = contextFile ? JSON.parse(await readFile(contextFile, 'utf8')) : undefined;
    const result = await runExperiment(parent, workload(profile, shape), { context });
    console.log(JSON.stringify({ root: result.root, report: result.report }));
  } else if (command === 'summarize') console.log(JSON.stringify(await summarize(args)));
  else throw new Error('usage');
} catch (error) {
  // Do not emit OS/provider errors or paths from potentially sensitive inputs.
  console.error(JSON.stringify({ error: error?.code?.startsWith('lab_') ? error.code : 'lab_experiment_failed' }));
  process.exitCode = 1;
}
