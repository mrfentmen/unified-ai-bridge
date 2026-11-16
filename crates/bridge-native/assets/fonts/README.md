# Embedded report fonts

Noto Sans Regular and Bold are embedded unchanged into generated PDF reports.
They are bundled, rather than selected from the user's system, so layout and
glyph metrics do not depend on the host's installed fonts. The main application
license does not replace the fonts' SIL Open Font License 1.1 in [OFL.txt](OFL.txt).
The fonts may be redistributed and embedded under that license. No font names
or outlines have been modified.

Retrieved November 16, 2026 from the Noto project's `noto-fonts` repository:

- https://raw.githubusercontent.com/notofonts/noto-fonts/main/hinted/ttf/NotoSans/NotoSans-Regular.ttf
- https://raw.githubusercontent.com/notofonts/noto-fonts/main/hinted/ttf/NotoSans/NotoSans-Bold.ttf
- https://raw.githubusercontent.com/notofonts/noto-fonts/main/LICENSE

SHA256 of the committed files:

```
b85c38ecea8a7cfb39c24e395a4007474fa5a4fc864f6ee33309eb4948d232d5  NotoSans-Regular.ttf
c976e4b1b99edc88775377fcc21692ca4bfa46b6d6ca6522bfda505b28ff9d6a  NotoSans-Bold.ttf
0dab92d0544f7b233403f14b84a663bdbfa746982eda629e7f4f9ffe1b036feb  OFL.txt
```

The report implementation validates both fonts with `ttf-parser`, embeds their
TrueType data with an explicit character-to-glyph map, and supplies a ToUnicode
map for extraction. Only a declared simple Latin subset is accepted; bundling
these fonts is not a claim to support every script present in their glyph sets.
