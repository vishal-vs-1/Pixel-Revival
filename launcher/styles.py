"""Apply the system palette to Tk widgets and themes."""

from launcher.palette import java_color, nes_color
from systems.contracts import NES


class LauncherStyles:
    def _apply_system_palette(self):
        color=nes_color if self.visible_system_id==NES else java_color
        self.bg, self.panel, self.card=map(color, ('#0c121c','#121c29','#192636'))
        self.root.configure(bg=self.bg)
        for widget,options in self._launcher_widget_colors:
            widget.configure(**{name:color(value) for name,value in options.items()})
        self.tab_navigation.set_palette(color)
        self.row_actions.set_palette(color)
        self._configure_launcher_styles(color)

    def _configure_launcher_styles(self, color):
        style = self._launcher_style
        style.configure('.', font=('Segoe UI',10), background=self.panel, foreground=color('#dce7f5'))
        # Clam's default active/disabled maps otherwise override the dark palette.
        style.map('.',background=[('disabled',self.panel),('active',self.card)],
                  foreground=[('disabled',color('#7994ac')),('active',color('#eff5fc'))])
        style.configure('TCheckbutton',background=self.panel,foreground=color('#dce7f5'),
                        indicatorbackground=self.card,indicatorforeground=color('#102720'),
                        bordercolor=color('#617f98'),lightcolor=self.card,darkcolor=self.card)
        style.map('TCheckbutton',background=[('disabled',self.panel),('active',color('#263649'))],
                  foreground=[('disabled',color('#7994ac')),('active',color('#eff5fc'))],
                  indicatorbackground=[('disabled',color('#263649')),('selected',color('#84e4c7')),('active',color('#354b60'))],
                  indicatorforeground=[('disabled',color('#7994ac')),('selected',color('#102720'))])
        style.configure('TButton',background=color('#263649'),foreground=color('#dce7f5'),padding=(13,9),borderwidth=0)
        style.map('TButton',background=[('active',color('#354b60'))])
        style.configure('Accent.TButton',background=color('#84e4c7'),foreground=color('#102720'),font=('Segoe UI',11,'bold'))
        style.map('Accent.TButton',background=[('disabled',color('#263649')),('active',color('#a8f5df'))],
                  foreground=[('disabled',color('#7994ac')),('!disabled',color('#102720'))])
        style.configure('Delete.TButton',foreground=color('#ffb6b6'))
        style.map('Delete.TButton',foreground=[('disabled',color('#7994ac')),('active',color('#ffd0d0'))])
        for orientation in ('Vertical','Horizontal'):
            style.configure(orientation+'.TScrollbar',background=color('#4a6378'),troughcolor=self.bg,
                            arrowcolor=color('#b7c9d9'),bordercolor=color('#263649'),lightcolor=color('#4a6378'),darkcolor=color('#4a6378'))
            style.map(orientation+'.TScrollbar',background=[('active',color('#617f98'))])
        style.configure('TCombobox',fieldbackground=self.card,background=color('#2a3d50'),foreground=color('#e8f1fc'),arrowcolor=color('#91b2c6'),padding=6)
        style.map('TCombobox',fieldbackground=[('readonly',self.card)],foreground=[('readonly',color('#e8f1fc'))])
        style.configure('Treeview',background=self.panel,foreground=color('#dce7f5'),fieldbackground=self.panel,rowheight=54,borderwidth=0,font=('Segoe UI',11))
        style.map('Treeview',background=[('selected',color('#254b49'))],foreground=[('selected',color('#c4ffed'))])
        style.configure('Treeview.Heading',background=self.panel,foreground=color('#7994ac'),font=('Segoe UI',9,'bold'),padding=(10,12),borderwidth=0)
        style.configure('Horizontal.TProgressbar',background=color('#84e4c7'),troughcolor=color('#243243'),borderwidth=0,thickness=4)
        style.configure('TNotebook',background=self.panel,borderwidth=0)
        style.configure('TNotebook.Tab',background=self.card,foreground=color('#8ea5b9'),padding=(16,9))
        style.map('TNotebook.Tab',background=[('selected',color('#254b49')),('active',color('#263649'))],
                  foreground=[('selected',color('#c4ffed')),('active',color('#eff5fc'))])
        style.configure('Content.TNotebook',background=self.panel,borderwidth=0)
        style.layout('Content.TNotebook.Tab',[])
        style.map('Treeview.Heading',background=[('active',color('#263649'))],foreground=[('active',color('#eff5fc'))])
        self.root.option_add('*TCombobox*Listbox.background',self.card)
        self.root.option_add('*TCombobox*Listbox.foreground',color('#e8f1fc'))
        style.layout('System.TRadiobutton',style.layout('TButton'))
        style.configure('System.TFrame',background=self.card)
        style.configure('System.TRadiobutton',padding=(4,9),anchor='center',
                        font=('Segoe UI',9,'bold'),background=self.card,
                        foreground=color('#dce7f5'),focuscolor=color('#617f98'),borderwidth=0)
        style.map('System.TRadiobutton',
                  background=[('selected','active',color('#a8f5df')),
                              ('selected',color('#84e4c7')),('active',color('#354b60'))],
                  foreground=[('selected',color('#102720')),('active',color('#eff5fc'))])
