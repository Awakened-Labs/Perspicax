// One of each interactive control the C1 kill criterion's denominator names.
//
// The denominator matters more than the numerator here: an "interactive
// control" is fixed independently of anything Wine reports, so the set below is
// the whole list -- Button, Edit, CheckBox, ComboBox, List, ListItem, MenuItem,
// Tab, Tree -- and nothing is added after seeing the data.
//
// Built with the mcs.exe that ships inside wine-mono, against the same
// assemblies it runs on. That is deliberate: a target cross-compiled against
// Microsoft's reference assemblies would test the compiler as well as the
// question, and the question is only whether a managed accessible tree exists.
using System;
using System.Drawing;
using System.Windows.Forms;

public class Preflight : Form
{
    [STAThread]
    public static void Main()
    {
        Application.EnableVisualStyles();
        Application.Run(new Preflight());
    }

    public Preflight()
    {
        Text = "PreflightTarget";
        Size = new Size(520, 460);

        var menu = new MenuStrip();
        var file = new ToolStripMenuItem("File");
        file.DropDownItems.Add(new ToolStripMenuItem("OpenItem"));
        menu.Items.Add(file);
        Controls.Add(menu);

        // THE DISCRIMINATOR. These two properties exist only in managed code.
        // oleacc's own class handlers cannot know them: a BUTTON-class window
        // asked by the stub path reports its window text and an inferred role.
        // So if the probe reads back MANAGED_SENTINEL/slider, a managed provider
        // answered WM_GETOBJECT. If it reads back "PushMe"/client, oleacc did.
        Add(new Button   { Text = "PushMe",   Name = "btn",   Left = 20, Top = 40,
                           AccessibleName = "MANAGED_SENTINEL",
                           AccessibleRole = AccessibleRole.Slider });
        Add(new TextBox  { Text = "EditHere", Name = "edit",  Left = 20, Top = 80  });
        Add(new CheckBox { Text = "CheckMe",  Name = "check", Left = 20, Top = 120 });

        var combo = new ComboBox { Name = "combo", Left = 20, Top = 160 };
        combo.Items.AddRange(new object[] { "ComboA", "ComboB" });
        Add(combo);

        var list = new ListBox { Name = "list", Left = 20, Top = 200, Height = 60,
                                 AccessibleName = "MANAGED_LIST_SENTINEL" };
        list.Items.AddRange(new object[] { "ListItemA", "ListItemB" });
        Add(list);

        var tabs = new TabControl { Name = "tabs", Left = 240, Top = 40, Size = new Size(240, 120) };
        tabs.TabPages.Add(new TabPage("TabOne"));
        tabs.TabPages.Add(new TabPage("TabTwo"));
        Add(tabs);

        var tree = new TreeView { Name = "tree", Left = 240, Top = 180, Size = new Size(240, 120) };
        var root = tree.Nodes.Add("TreeRoot");
        root.Nodes.Add("TreeChild");
        tree.ExpandAll();
        Add(tree);
    }

    void Add(Control c) { Controls.Add(c); }
}
