#!/usr/bin/env node
// Stands in for semsearch in the extension's tests. `search ... <query>`
// sleeps for the number of milliseconds after the last `@` in the query, then
// prints one hit named after the query, at /x/<query>. A query ending in
// `#files` gets three hits instead, two of them in one file. Each start is
// logged to $FAKE_LOG, and the arguments of the last call to $FAKE_LOG.args.
const fs = require('fs');
const query = process.argv[process.argv.length - 1];
fs.writeFileSync(`${process.env.FAKE_LOG}.args`, JSON.stringify(process.argv.slice(2)));
const hit = (file, line, title) => ({ source: 'code', url: `vscode://file/x/${file}:${line}:1`, title, chunk: title, similarity: 0.5, rerank_score: 0.5, updated: 0 });
fs.appendFileSync(process.env.FAKE_LOG, `start ${query}\n`);
const ms = Number((query.match(/@(\d+)$/) || [, '0'])[1]);
setTimeout(() => {
  fs.appendFileSync(process.env.FAKE_LOG, `end ${query}\n`);
  const hits = query.endsWith('#files') ? [hit('a.rs', 10, 'a10'), hit('b.rs', 2, 'b2'), hit('a.rs', 3, 'a3')] : [hit(query, 1, query)];
  for (const h of hits) console.log(JSON.stringify(h));
}, ms);
