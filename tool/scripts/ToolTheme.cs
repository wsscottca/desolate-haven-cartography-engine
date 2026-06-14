using Godot;

namespace DesolateHaven.Cartography;

/// The Desolate Haven UI theme, per the Visual Style Guide (web/mapgen4/style.html): deep
/// brown rounded panels, a gold brand accent, ghost buttons, IM Fell English SC for headers /
/// titles and Alegreya for body text. Fonts load from res://fonts/ if present and degrade to the
/// engine default otherwise, so a missing .ttf never breaks the tool.
public static class ToolTheme
{
    public static readonly Color Bg     = Color.FromHtml("14110E"); // page backdrop
    public static readonly Color Bg2    = Color.FromHtml("1E1A15"); // surface
    public static readonly Color Bg3    = Color.FromHtml("272118"); // raised panel
    public static readonly Color Rule   = Color.FromHtml("3A3326"); // hairline borders
    public static readonly Color Ink    = Color.FromHtml("ECE3D0"); // primary text
    public static readonly Color InkDim = Color.FromHtml("B9AE97"); // muted text
    public static readonly Color Gold   = Color.FromHtml("C7A24B"); // brand accent

    /// IM Fell English SC — titles, section headers, tool labels. Null if the .ttf is absent.
    public static FontFile Display { get; private set; }
    /// Alegreya — body / values. Null if absent.
    public static FontFile Body { get; private set; }

    public static Theme Build()
    {
        Display = LoadFont("res://fonts/IMFeENsc28P.ttf");
        Body = LoadFont("res://fonts/Alegreya-Variable.ttf");

        var t = new Theme { DefaultFontSize = 14 };
        if (Body != null) t.DefaultFont = Body;

        t.SetStylebox("panel", "PanelContainer", Panel(Bg3, Rule, 10, 12));

        // Ghost buttons: faint surface fill + hairline; gold border/text on hover/press.
        t.SetStylebox("normal", "Button", Panel(new Color(Bg2, 0.55f), Rule, 6, 7));
        t.SetStylebox("hover", "Button", Panel(Bg3, Gold, 6, 7));
        t.SetStylebox("pressed", "Button", Panel(new Color(Gold, 0.18f), Gold, 6, 7));
        t.SetStylebox("focus", "Button", new StyleBoxEmpty());
        t.SetColor("font_color", "Button", Ink);
        t.SetColor("font_hover_color", "Button", Gold);
        t.SetColor("font_pressed_color", "Button", Gold);
        t.SetColor("font_focus_color", "Button", Ink);
        t.SetColor("font_hover_pressed_color", "Button", Gold);

        // OptionButton mirrors Button chrome.
        t.SetStylebox("normal", "OptionButton", Panel(new Color(Bg2, 0.55f), Rule, 6, 7));
        t.SetStylebox("hover", "OptionButton", Panel(Bg3, Gold, 6, 7));
        t.SetStylebox("pressed", "OptionButton", Panel(new Color(Gold, 0.18f), Gold, 6, 7));
        t.SetColor("font_color", "OptionButton", Ink);

        t.SetColor("font_color", "Label", Ink);
        t.SetColor("font_color", "CheckButton", Ink);
        return t;
    }

    /// A gold small-caps section header (IM Fell English SC if available), e.g. "TERRAIN TOOLS".
    public static Label Header(string text)
    {
        var l = new Label { Text = text };
        l.AddThemeColorOverride("font_color", Gold);
        l.AddThemeFontSizeOverride("font_size", 14);
        if (Display != null) l.AddThemeFontOverride("font", Display);
        return l;
    }

    private static StyleBoxFlat Panel(Color bg, Color border, int radius, int pad)
    {
        var sb = new StyleBoxFlat { BgColor = bg, BorderColor = border };
        sb.SetBorderWidthAll(1);
        sb.SetCornerRadiusAll(radius);
        sb.SetContentMarginAll(pad);
        return sb;
    }

    private static FontFile LoadFont(string path)
    {
        return ResourceLoader.Exists(path) ? ResourceLoader.Load<FontFile>(path) : null;
    }
}
