# semsearch-rerank, the jina-reranker-v3.5 server. `python` must carry a torch
# that can reach the GPU; nixcfg passes the CUDA interpreter it builds for
# ComfyUI, so torch is not compiled a second time. flash-attn takes torch's
# CUDA capabilities, so it is compiled for the same GPUs only.
{
  lib,
  python,
  makeBinaryWrapper,
  runCommand,
}:
let
  env = python.withPackages (ps: [
    ps.torch
    ps.transformers
    ps.safetensors
    ps.numpy
    ps.tokenizers
    ps.flash-attn
  ]);
in
runCommand "semsearch-rerank"
  {
    nativeBuildInputs = [ makeBinaryWrapper ];
    meta = {
      description = "jina-reranker-v3.5 behind llama-server's /v1/rerank API";
      mainProgram = "semsearch-rerank";
      platforms = lib.platforms.linux;
    };
  }
  ''
    mkdir -p $out/bin $out/share/semsearch-rerank
    cp ${../rerank/server.py} $out/share/semsearch-rerank/server.py
    makeBinaryWrapper ${lib.getExe env} $out/bin/semsearch-rerank \
      --add-flag $out/share/semsearch-rerank/server.py
  ''
