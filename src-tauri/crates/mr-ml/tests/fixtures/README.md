# Происхождение фикстур

* `speaker1_a_cn_16k.wav`, `speaker1_b_cn_16k.wav`, `speaker2_a_cn_16k.wav` —
  официальные демо-записи голосов (китайский, 16 кГц) из тулкита **3D-Speaker**
  (Alibaba Inc., лицензия **Apache-2.0**: https://github.com/modelscope/3D-Speaker ,
  примеры speaker-verification в репозитории и датасетах 3D-Speaker на ModelScope).
  Используются тестом `onnx_real::eres2netv2_parity_with_official_reference`
  для проверки совпадения с официальным эталоном 3D-Speaker.
* `eres2netv2_reference.json` — эталонные 192-мерные эмбеддинги 3D-Speaker
  из тех же WAV; производный материал, распространяется вместе с ними по Apache-2.0.
