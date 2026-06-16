#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// Shared themed UI helpers for the DHCE editor surfaces (sidebar dock + viewport overlays). Keeps the
/// three surfaces visually consistent with the game parchment theme (ToolTheme) and DRY: one cached
/// theme and a collapsible-section building block used everywhere.
public static class DhceUi
{
    private static Theme _theme;

    /// The cached game theme (builds once; also loads ToolTheme.Display/Body fonts).
    public static Theme Theme
    {
        get { _theme ??= ToolTheme.Build(); return _theme; }
    }

    /// A collapsible section: a gold ▸/▾ toggle header that shows/hides a content VBox. Returns the
    /// content container so the caller adds the section's controls to it. Append sections in order to
    /// `parent`; each remembers its own open/closed state.
    public static VBoxContainer Section(Container parent, string title, bool startOpen = true)
    {
        _ = Theme; // ensure ToolTheme fonts are loaded before we reference Display
        parent.AddChild(new HSeparator());
        var head = new Button
        {
            ToggleMode = true,
            ButtonPressed = startOpen,
            Flat = true,
            Alignment = HorizontalAlignment.Left,
            SizeFlagsHorizontal = Control.SizeFlags.ExpandFill,
        };
        head.AddThemeColorOverride("font_color", ToolTheme.Gold);
        head.AddThemeColorOverride("font_hover_color", ToolTheme.Gold);
        head.AddThemeColorOverride("font_pressed_color", ToolTheme.Gold);
        head.AddThemeFontSizeOverride("font_size", 14);
        if (ToolTheme.Display != null) head.AddThemeFontOverride("font", ToolTheme.Display);
        parent.AddChild(head);

        var content = new VBoxContainer { SizeFlagsHorizontal = Control.SizeFlags.ExpandFill, Visible = startOpen };
        parent.AddChild(content);

        void Sync() => head.Text = (content.Visible ? "▾  " : "▸  ") + title;
        head.Toggled += on => { content.Visible = on; Sync(); };
        Sync();
        return content;
    }

    /// A panel-level collapse toggle: a gold ▸/▾ title button that shows/hides `body`. Returns the
    /// toggle button. Used to minimize a whole overlay panel down to its always-on header.
    public static Button CollapseToggle(Container parent, string title, Control body, bool startOpen = true)
    {
        _ = Theme;
        var head = new Button
        {
            ToggleMode = true,
            ButtonPressed = startOpen,
            Flat = true,
            Alignment = HorizontalAlignment.Left,
            SizeFlagsHorizontal = Control.SizeFlags.ExpandFill,
        };
        head.AddThemeColorOverride("font_color", ToolTheme.Gold);
        head.AddThemeColorOverride("font_hover_color", ToolTheme.Gold);
        head.AddThemeColorOverride("font_pressed_color", ToolTheme.Gold);
        head.AddThemeFontSizeOverride("font_size", 14);
        if (ToolTheme.Display != null) head.AddThemeFontOverride("font", ToolTheme.Display);
        parent.AddChild(head);
        body.Visible = startOpen;
        void Sync() => head.Text = (body.Visible ? "▾  " : "▸  ") + title;
        head.Toggled += on => { body.Visible = on; Sync(); };
        Sync();
        return head;
    }

    /// A muted (InkDim) wrap-friendly label.
    public static Label Dim(string text)
    {
        var l = new Label { Text = text, AutowrapMode = TextServer.AutowrapMode.WordSmart };
        l.AddThemeColorOverride("font_color", ToolTheme.InkDim);
        return l;
    }
}
#endif
