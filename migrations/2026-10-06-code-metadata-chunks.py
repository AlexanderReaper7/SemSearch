# /// script
# dependencies = ["sqlite-vec==0.1.6"]
# ///
"""Deletes the metadata chunk of every code document from a hister vector
store, for hister 5b0b0e6d, which no longer makes one (docs/decisions.md,
"Code pieces get no metadata vector").

    uv run migrations/2026-10-06-code-metadata-chunks.py <hister data>/vectors.sqlite3 [--apply]

Without --apply it only counts. Run it with hister stopped and before a hister
from 5b0b0e6d on has embedded anything: before, chunk 0 of a code document is
always its metadata chunk; after, it can be a body chunk. It refuses when any
chunk 0 of a code document does not start with "title: ".

sqlite-vec is pinned to the version hister bundles
(server/vectorstore/sqlitevec, v0.1.6), so the vec0 table is written as hister
reads it.
"""
import sqlite3
import sys

import sqlite_vec

db = sqlite3.connect(sys.argv[1])
db.enable_load_extension(True)
sqlite_vec.load(db)
code = "doc_id LIKE 'vscode://file/%' AND chunk_idx = 0"
keys = [k for (k,) in db.execute(f"SELECT chunk_key FROM chunk_meta WHERE {code}")]
other = db.execute(f"SELECT count(*) FROM chunk_meta WHERE {code} AND chunk_text NOT LIKE 'title: %'").fetchone()[0]
docs = db.execute("SELECT count(DISTINCT doc_id) FROM chunk_meta WHERE doc_id LIKE 'vscode://file/%'").fetchone()[0]
print(f"code documents: {docs}, chunk 0: {len(keys)}, chunk 0 not a metadata chunk: {other}")
if other:
    sys.exit("refusing: some chunk 0 is not a metadata chunk")
if "--apply" in sys.argv:
    with db:
        db.executemany("DELETE FROM embeddings WHERE chunk_key = ?", [(k,) for k in keys])
        db.executemany("DELETE FROM chunk_meta WHERE chunk_key = ?", [(k,) for k in keys])
    left = db.execute("SELECT count(*) FROM chunk_meta").fetchone()[0], db.execute("SELECT count(*) FROM embeddings").fetchone()[0]
    print(f"deleted {len(keys)}; chunks left: {left[0]} in chunk_meta, {left[1]} in embeddings")
