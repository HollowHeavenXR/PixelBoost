# Third-party notices

## Real-ESRGAN weights (Models/pbnn/*.pbnn)

The network weights in `pbnn/` are the official checkpoints from
[xinntao/Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN), converted to
PixelBoost's own fp16 `.pbnn` format by `pixelboost convert-all` (the
PyTorch checkpoint reader and the network architectures are PixelBoost's own
Rust code in `rust/crates/pb-nn`; no Real-ESRGAN code is used):

- `photo-x4.pbnn` <- `RealESRGAN_x4plus.pth`
- `anime-x4.pbnn` <- `RealESRGAN_x4plus_anime_6B.pth`
- `clean-x4.pbnn` <- `RealESRNet_x4plus.pth`
- `fast-x4.pbnn` <- `realesr-general-x4v3.pth`
- `fast-denoise-x4.pbnn` <- `realesr-general-wdn-x4v3.pth`
- `native-x2.pbnn` <- `RealESRGAN_x2plus.pth`
- `lineart-x4.pbnn` <- `realesr-animevideov3.pth`

```
BSD 3-Clause License

Copyright (c) 2021, Xintao Wang
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Noto fonts (rust/crates/pb-core/fonts)

The text tool bundles NotoSans-Regular, NotoSans-Black, NotoSerif-Bold,
NotoSerifDisplay-BlackItalic and NotoSansMono-Bold from the Noto project
(Google), licensed under the SIL Open Font License 1.1 — full text in
`rust/crates/pb-core/fonts/OFL.txt`.
