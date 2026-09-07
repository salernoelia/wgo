# Whisper native assets and attribution

`mel_128.npy` is the unmodified `mel_128` array extracted from OpenAI Whisper's
`whisper/assets/mel_filters.npz` (https://github.com/openai/whisper), MIT licensed.
It contains the reference Slaney-normalized 128-bin mel filters.

The native Rust architecture and audio preprocessing follow Apple's MIT-licensed
MLX Whisper reference: https://github.com/ml-explore/mlx-examples/tree/main/whisper.
The model is https://huggingface.co/mlx-community/whisper-large-v3-turbo-q4.
Its tokenizer and generation metadata come from openai/whisper-large-v3-turbo.

See LICENSE-openai and LICENSE-apple for the upstream licenses.
