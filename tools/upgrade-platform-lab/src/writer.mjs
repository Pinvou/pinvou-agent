// A real child-process SQLite/WAL writer, using synthetic records exclusively.
import { DatabaseSync } from 'node:sqlite';
import { openSync, writeFileSync, fsyncSync, closeSync } from 'node:fs';
import path from 'node:path';

let db, timer, frozen = false, sequence = 0, epoch, root, unsavable;
function fail() { process.send?.({ type: 'refused' }); }
process.on('message', message => {
  try {
    if (message?.type === 'start' && !db && !epoch) {
      ({ root, epoch, unsavable } = message);
      db = new DatabaseSync(path.join(root, 'state.sqlite'));
      db.exec('PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS writes(sequence INTEGER PRIMARY KEY,payload TEXT NOT NULL)');
      sequence = Number(db.prepare('SELECT coalesce(max(sequence),0) AS n FROM writes').get().n);
      const insert = db.prepare('INSERT INTO writes VALUES(?,?)');
      const write = () => { if (!frozen) { insert.run(++sequence, `synthetic-${sequence}`); timer = setTimeout(write, 5); } };
      write();
      process.send({ type: 'ready', epoch });
    } else if (message?.type === 'freeze' && message.epoch === epoch && !frozen && db) {
      if (unsavable) { fail(); return; }
      frozen = true;
      clearTimeout(timer);
      const checkpoint = db.prepare('PRAGMA wal_checkpoint(TRUNCATE)').get();
      if (Number(checkpoint.busy) !== 0) throw new Error('busy');
      // This synthetic database belongs exclusively to this writer. Leave a
      // complete standalone database, without copying live WAL/SHM lock state.
      if (db.prepare('PRAGMA journal_mode=DELETE').get().journal_mode !== 'delete') throw new Error('journal');
      db.close(); db = undefined;
      const file = openSync(path.join(root, 'frozen.json'), 'wx', 0o600);
      try { writeFileSync(file, JSON.stringify({ epoch, sequence })); fsyncSync(file); }
      finally { closeSync(file); }
      process.send({ type: 'frozen', epoch, sequence });
    } else if (message?.type === 'stop' && message.epoch === epoch) {
      clearTimeout(timer); db?.close(); db = undefined; process.exit(0);
    } else { fail(); }
  } catch { fail(); }
});
process.on('disconnect', () => { clearTimeout(timer); try { db?.close(); } finally { process.exit(0); } });
