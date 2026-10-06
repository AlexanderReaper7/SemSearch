#!/usr/bin/env node
// Stands in for semsearch in the extension's tests. `search ... <query>`
// sleeps for the number of milliseconds after the last `@` in the query, then
// prints one hit named after the query. Each start is logged to $FAKE_LOG.
const fs = require('fs');
const query = process.argv[process.argv.length - 1];
fs.appendFileSync(process.env.FAKE_LOG, `start ${query}\n`);
const ms = Number((query.match(/@(\d+)$/) || [, '0'])[1]);
setTimeout(() => {
  fs.appendFileSync(process.env.FAKE_LOG, `end ${query}\n`);
  console.log(JSON.stringify({ source: 'code', url: `vscode://file/x/${query}:1:1`, title: query, chunk: query, similarity: 0.5, rerank_score: 0.5, updated: 0 }));
}, ms);
