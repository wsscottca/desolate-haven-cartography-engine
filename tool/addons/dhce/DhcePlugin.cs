#if TOOLS
using Godot;

namespace DesolateHaven.Cartography;

/// In-editor entry point for the DHCE authoring tool — the editor-plugin reshell (ADR 0005).
/// Editor-only (`#if TOOLS`): it adds a dock that will drive a selected `DhceWorld` node. R1 is the
/// scaffold; later stages add gen-on-demand (R2), viewport picking + the 3D-sphere brush (R3), the
/// full tool dock (R4), and `DhceWorldState` persistence (R5).
[Tool]
public partial class DhcePlugin : EditorPlugin
{
    private Control _dock;

    public override void _EnterTree()
    {
        _dock = BuildDock();
        AddControlToDock(DockSlot.RightUl, _dock);
    }

    public override void _ExitTree()
    {
        if (_dock != null)
        {
            RemoveControlFromDocks(_dock);
            _dock.QueueFree();
            _dock = null;
        }
    }

    private static Control BuildDock()
    {
        var root = new VBoxContainer { Name = "DHCE" };
        var title = new Label { Text = "DHCE Cartographer" };
        title.AddThemeFontSizeOverride("font_size", 16);
        root.AddChild(title);
        root.AddChild(new Label
        {
            Text = "Add a DhceWorld node, then Generate. (Scaffold — tools arrive in later stages.)",
            AutowrapMode = TextServer.AutowrapMode.WordSmart,
        });
        return root;
    }
}
#endif
