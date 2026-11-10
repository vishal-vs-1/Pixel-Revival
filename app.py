"""Persistent desktop game library and local artwork preparation."""
import json
import os
import queue
import subprocess
import sys
import threading
import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tkinter as tk
from tkinter import ttk, filedialog, messagebox

from engine import Library, ROOT, atomic_json
from app_paths import APP_ROOT, DATA_ROOT, FROZEN, RESOURCE_ROOT
from gpu_policy import settings as gpu_settings, save_settings as save_gpu_settings, devices as gpu_devices
from ui_widgets import ScrollableFrame, TabNavigation, LibraryRowActions
from ui_palette import capture_widget_colors, java_color, nes_color
from library_preview import load_java_preview
from control_profiles import DEFAULT_SLOT_ID
from control_schema import JME_CONTROL_SCHEMA, NES_CONTROL_SCHEMA
from control_profile_dialog import ControlProfileDialog
from systems.nes.controls import NesControlProfileStore
from systems.application import ApplicationLibrary
from systems.contracts import GameRef, ImportRequest, JAVA_ME, NES
from systems.java_me import JavaMeSystem
from systems.nes.library import NesLibrary
from systems.nes.integration import NesCaptureIntegration
from systems.nes.runtime_setup import NesRuntimeLocator
from systems.app_settings import ApplicationSettingsStore
from systems.ui_events import ApplicationEvent
from graphics.backend_preference import (
    COMMON_BACKEND, LEGACY_BACKEND, GraphicsBackendPreferenceStore,
)
from graphics.jme_runtime import JmeGraphicsRuntimeLocator
from graphics.launch import JmeGraphicsLaunchPolicy

GPU_CHOICES={'Auto · dedicated first':'auto','Dedicated GPU':'dedicated','Integrated GPU':'integrated','Windows default':'system'}
GRAPHICS_BACKEND_CHOICES={
    'Shared GPU · common-v1': COMMON_BACKEND,
    'Legacy GPU · compatibility': LEGACY_BACKEND,
}
SYSTEM_LABELS={JAVA_ME:'Java ME',NES:'NES'}


class App:
    def __init__(self, root):
        self.root = root
        self.nes_control_profiles = NesControlProfileStore(
            DATA_ROOT / 'config' / 'nes-control-profiles.json', atomic_json)
        nes_runtime = NesRuntimeLocator(APP_ROOT, packaged=FROZEN).locate()
        self.graphics_backend_store = GraphicsBackendPreferenceStore(
            DATA_ROOT / 'config' / 'graphics-backend.json', atomic_json)
        preferred_manifest = getattr(nes_runtime, 'graphics_manifest', None)
        self.graphics_runtime_locator = JmeGraphicsRuntimeLocator(
            RESOURCE_ROOT, packaged=FROZEN, preferred_manifest=preferred_manifest)
        self.graphics_launch_policy = JmeGraphicsLaunchPolicy(
            self.graphics_backend_store, self.graphics_runtime_locator)
        java_library = Library(graphics_launch_policy=self.graphics_launch_policy)
        java = JavaMeSystem(java_library, demo=ROOT / 'fixtures' / 'RevivalDemo.jar')

        def register_nes_artwork(folder, game, recovering=False):
            recipe, ai_recipe = java_library.recipe_for_backend(game["backend"])
            integration = NesCaptureIntegration(
                game["id"], recipe,
                expected_palette_profile=game.get("palette_profile"),
                pack_scale=game.get("render_scale", 2),
            )
            return java_library.asset_service.register_game(
                folder, recipe, game=game, ai_recipe=ai_recipe,
                integration=integration,
            )

        # Keep the JME paths intact and give NES its independently owned storage tree.
        nes = NesLibrary(DATA_ROOT / 'nes', asset_service=java_library.asset_service,
                         control_profiles=self.nes_control_profiles, runtime=nes_runtime,
                         register_artwork=register_nes_artwork)
        self.library = ApplicationLibrary((java, nes), java_library.asset_service)
        self.app_settings = ApplicationSettingsStore(
            DATA_ROOT / 'config' / 'application.json', atomic_json)
        self.visible_system_id = self.app_settings.load_visible_system()
        self.messages = queue.Queue()
        self._asset_stats = {}
        self._catalog = []
        self._catalog_filter_pending = None
        self._preview_worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix='library-preview')
        self._preview_request = None
        self._preview_future = None
        self._preview_count = None
        self.library.set_asset_listener(self._on_asset_stats)
        self.busy = False
        self.deleting_games = {}
        self.player_refs = {}
        self._pending_player_states = set()
        self._delete_workers = ThreadPoolExecutor(max_workers=2,thread_name_prefix='delete-game')
        self.pending_imports = deque()
        self.preparation_paused = False
        self.players: dict[str, tuple[object, Path]] = {}
        self.settings_dialog = None
        self.control_profile_choice_ids = {}
        self.control_profile_problem = None
        root.title('JAR Revival — Java ME & NES')
        root.geometry('1140x740')
        root.minsize(980, 680)
        self.bg, self.panel, self.card = '#0c121c', '#121c29', '#192636'
        root.configure(bg=self.bg)
        self._launcher_style=ttk.Style(root)
        self._launcher_style.theme_use('clam')
        self._configure_launcher_styles(java_color)
        rail=tk.Frame(root,bg=self.bg,width=168,padx=20,pady=26)
        rail.pack(side='left',fill='y');rail.pack_propagate(False)
        tk.Label(rail,text='JAR\nREVIVAL',bg=self.bg,fg='#84e4c7',font=('Segoe UI',24,'bold'),justify='left').pack(anchor='w')
        tk.Label(rail,text='KEYPAD CLASSICS\nON YOUR DESKTOP',bg=self.bg,fg='#6e879e',font=('Segoe UI',8),justify='left').pack(anchor='w',pady=(8,32))
        tk.Label(rail,text='  ◉  Your library',bg='#1b3339',fg='#9cf0d5',font=('Segoe UI',10,'bold'),padx=10,pady=12).pack(fill='x')
        tk.Label(rail,text='GAME SYSTEM',bg=self.bg,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(18,5))
        self.system_choice=tk.StringVar(value=self.visible_system_id)
        switch=ttk.Frame(rail,style='System.TFrame',padding=2)
        switch.pack(fill='x')
        self.system_buttons={}
        for column,(system,label) in enumerate(SYSTEM_LABELS.items()):
            switch.columnconfigure(column,weight=1,uniform='system')
            button=ttk.Radiobutton(switch,text=label,value=system,variable=self.system_choice,
                                   command=self.set_visible_system,style='System.TRadiobutton')
            button.grid(row=0,column=column,sticky='ew')
            self.system_buttons[system]=button
        self.settings_button=ttk.Button(rail,text='Settings',command=self.open_settings)
        self.settings_button.pack(fill='x',pady=(10,0))
        self.demo_button=ttk.Button(rail,text='Import demo',command=self.import_demo)
        self.demo_button.pack(side='bottom',fill='x',pady=(12,0))
        tk.Label(rail,text='LOCAL & PRIVATE\nNo accounts. No API fees.',bg=self.bg,fg='#6e879e',font=('Segoe UI',8),justify='left').pack(side='bottom',anchor='w')
        main=tk.Frame(root,bg=self.bg,padx=20,pady=22);main.pack(fill='both',expand=True)
        header=tk.Frame(main,bg=self.bg);header.pack(fill='x',pady=(0,20))
        self.add_button=ttk.Button(header,text='+ Add game',style='Accent.TButton',command=self.import_game)
        self.add_button.pack(side='right')
        self.collection_title=tk.StringVar(value='Your Java ME collection')
        tk.Label(header,textvariable=self.collection_title,bg=self.bg,fg='#eff5fc',font=('Segoe UI',23,'bold')).pack(anchor='w')
        tk.Label(header,text='Prepared artwork. Original gameplay.',bg=self.bg,fg='#8ea5b9',font=('Segoe UI',10)).pack(anchor='w',pady=(4,0))
        body=tk.Frame(main,bg=self.bg);body.pack(fill='both',expand=True)
        body.columnconfigure(0,weight=4,minsize=260);body.columnconfigure(1,weight=6,minsize=390);body.rowconfigure(0,weight=1)
        library_card=tk.Frame(body,bg=self.panel,padx=12,pady=12);library_card.grid(row=0,column=0,sticky='nsew',padx=(0,14))
        tk.Label(library_card,text='SEARCH YOUR GAMES',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(0,7))
        searchrow=tk.Frame(library_card,bg=self.panel);searchrow.pack(fill='x',pady=(0,8))
        self.search=tk.StringVar()
        search=tk.Entry(searchrow,textvariable=self.search,bg=self.card,fg='#dce7f5',
                        insertbackground='white',selectbackground='#254b49',
                        selectforeground='#c4ffed',relief='flat',font=('Segoe UI',10))
        search.pack(side='left',fill='x',expand=True,ipady=8,padx=(0,8))
        ttk.Button(searchrow,text='↻',width=2,command=self.refresh).pack(side='right')
        library_list=tk.Frame(library_card,bg=self.panel);library_list.pack(fill='both',expand=True)
        library_list.columnconfigure(0,weight=1);library_list.rowconfigure(0,weight=1)
        self.tree=ttk.Treeview(library_list,height=4,columns=('actions',),show='tree headings',selectmode='browse')
        self.tree.heading('#0',text='GAME')
        self.tree.column('#0',width=165,minwidth=1,stretch=False)
        self.tree.heading('actions',text='')
        self.tree.column('actions',width=84,minwidth=84,stretch=False)
        self.library_scrollbar=ttk.Scrollbar(library_list,orient='vertical',command=self.tree.yview)
        self.tree.configure(yscrollcommand=self.library_scrollbar.set)
        self.tree.grid(row=0,column=0,sticky='nsew')
        self.library_scrollbar.grid(row=0,column=1,sticky='ns')
        self.row_actions=LibraryRowActions(self.tree,self.library_scrollbar,self.play_row,self.delete_row)
        tk.Label(library_card,text='Deleting a game keeps saved progress.',bg=self.panel,fg='#7994ac',font=('Segoe UI',8)).pack(pady=(8,0))
        self.tree.bind('<Double-1>',self.library_double_click)
        self.tree.bind('<<TreeviewSelect>>', self.selected)
        self.search.trace_add('write',self._schedule_catalog_filter)
        detail=tk.Frame(body,bg=self.panel,padx=20,pady=16);detail.grid(row=0,column=1,sticky='nsew')
        self.detail_title=tk.StringVar(value='Select a game')
        self.detail_subtitle=tk.StringVar(value='Add a game to get started.')
        tk.Label(detail,textvariable=self.detail_title,bg=self.panel,fg='#eff5fc',font=('Segoe UI',20,'bold'),anchor='w',justify='left',wraplength=420).pack(fill='x')
        tk.Label(detail,textvariable=self.detail_subtitle,bg=self.panel,fg='#8ea5b9',font=('Segoe UI',9),anchor='w',justify='left',wraplength=420).pack(fill='x',pady=(2,12))
        cover=tk.Frame(detail,bg=self.card,padx=14,pady=12);cover.pack(fill='x')
        preview=tk.Frame(cover,bg=self.card,width=96,height=96)
        preview.pack(side='left',padx=(0,12));preview.pack_propagate(False)
        self.cover=tk.Label(preview,text='JAR',bg=self.card,fg='#84e4c7',font=('Segoe UI',28,'bold'),width=0)
        self.cover.pack(fill='both',expand=True)
        info=tk.Frame(cover,bg=self.card);info.pack(fill='both',expand=True)
        self.cover_status=tk.StringVar(value='SAVED & READY')
        tk.Label(info,textvariable=self.cover_status,bg=self.card,fg='#84e4c7',font=('Segoe UI',9,'bold'),anchor='w').pack(fill='x')
        self.cache_info=tk.StringVar(value='No game selected')
        tk.Label(info,textvariable=self.cache_info,bg=self.card,fg='#dce7f5',font=('Segoe UI',10),anchor='w',justify='left',wraplength=280).pack(fill='x',pady=(5,4))
        self.compat=tk.StringVar(value='Compatibility: not checked')
        tk.Label(info,textvariable=self.compat,bg=self.card,fg='#8ea5b9',font=('Segoe UI',9),anchor='w',justify='left',wraplength=280).pack(fill='x')
        self.tabs=tabs=ttk.Notebook(detail,style='Content.TNotebook');tabs.pack(fill='both',expand=True)
        self.detail_views=[ScrollableFrame(tabs,bg=self.panel) for _ in range(4)]
        for view,title in zip(self.detail_views,('Play','Artwork','Compatibility','GPUs')):
            view.content.configure(padx=4,pady=10)
            tabs.add(view,text=title)
        self.tab_navigation=TabNavigation(detail,tabs,bg=self.panel)
        self.tab_navigation.pack(before=tabs,fill='x',pady=(16,8))
        play_panel,art_panel,compatibility_panel,gpu_panel=[view.content for view in self.detail_views]
        gpu_preferences=gpu_settings()
        renderer_preference=self.graphics_backend_store.load()
        tk.Label(gpu_panel,text='JAVAME GPU RENDERER',bg=self.panel,fg='#7994ac',
                 font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(3,5))
        self.graphics_backend_choice=ttk.Combobox(
            gpu_panel,values=list(GRAPHICS_BACKEND_CHOICES),state='readonly',width=29)
        backend_label=next((label for label,value in GRAPHICS_BACKEND_CHOICES.items()
                            if value==renderer_preference.backend),
                           'Legacy GPU · compatibility')
        self.graphics_backend_choice.set(backend_label)
        self.graphics_backend_choice.pack(anchor='w',pady=(0,4))
        self.graphics_backend_choice.bind('<<ComboboxSelected>>',self.set_graphics_backend)
        self.graphics_backend_info=tk.StringVar()
        self._refresh_graphics_backend_info(renderer_preference)
        tk.Label(gpu_panel,textvariable=self.graphics_backend_info,bg=self.panel,
                 fg='#9db7cc',font=('Segoe UI',8),justify='left',wraplength=420).pack(anchor='w',pady=(0,9))
        for title,key in [('GAME PLAYBACK GPU','render_gpu'),('ARTWORK CONVERSION GPU','enhance_gpu')]:
            tk.Label(gpu_panel,text=title,bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(3,5))
            combo=ttk.Combobox(gpu_panel,values=list(GPU_CHOICES),state='readonly',width=27)
            combo.set(next(label for label,value in GPU_CHOICES.items() if value==gpu_preferences[key]));combo.pack(anchor='w',pady=(0,8))
            combo.bind('<<ComboboxSelected>>',lambda event,k=key,c=combo:self.set_gpu(k,c.get()))
            setattr(self,key,combo)
        self.gpu_inventory=tk.StringVar(value='Detecting graphics devices…')
        self.active_gpu=tk.StringVar(value='The actual playback GPU appears here after launch.')
        tk.Label(gpu_panel,textvariable=self.gpu_inventory,bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w',pady=(3,5))
        tk.Label(gpu_panel,textvariable=self.active_gpu,bg=self.panel,fg='#84e4c7',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w')
        tk.Label(gpu_panel,text='Global settings. Playback changes apply on the next launch.\nWindows/driver overrides are reported using the actual GPU.',bg=self.panel,fg='#7994ac',font=('Segoe UI',8),justify='left',wraplength=420).pack(anchor='w',pady=(6,0))
        settings=tk.Frame(play_panel,bg=self.panel);settings.pack(fill='x',pady=(6,8));settings.columnconfigure(0,weight=1);settings.columnconfigure(1,weight=1)
        tk.Label(settings,text='DISPLAY DETAIL',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).grid(row=0,column=0,sticky='w')
        tk.Label(settings,text='DESKTOP VIEW',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).grid(row=0,column=1,sticky='w',padx=(12,0))
        self.quality=ttk.Combobox(settings,values=['Balanced · 2×','Detailed · 4×'],state='readonly',width=17)
        self.quality.set('Detailed · 4×');self.quality.grid(row=1,column=0,sticky='ew',pady=(5,0))
        self.quality.bind('<<ComboboxSelected>>',self.set_quality)
        self.desktop_fit_label=ttk.Label(settings,text='Fit desktop height',padding=(10,8))
        self.desktop_fit_label.grid(row=1,column=1,sticky='ew',padx=(12,0),pady=(5,0))
        self.aspect=tk.StringVar(value='4:3')
        self.aspect_combo=ttk.Combobox(settings,textvariable=self.aspect,
            values=('4:3','16:15'),state='readonly',width=13)
        self.aspect_combo.bind('<<ComboboxSelected>>',self.set_aspect)
        control_row=tk.Frame(play_panel,bg=self.panel);control_row.pack(fill='x',pady=(3,4))
        tk.Label(control_row,text='CONTROL PROFILE',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(0,5))
        control_actions=tk.Frame(control_row,bg=self.panel);control_actions.pack(fill='x')
        self.control_profile=tk.StringVar(value='Default')
        self.control_profile_combo=ttk.Combobox(control_actions,textvariable=self.control_profile,state='disabled',width=22)
        self.control_profile_combo.pack(side='left',fill='x',expand=True)
        self.control_profile_combo.bind('<<ComboboxSelected>>',self.set_control_profile)
        self.control_profile_info=tk.StringVar(value='Default controls are selected.')
        tk.Label(control_row,textvariable=self.control_profile_info,bg=self.panel,fg='#9db7cc',
                 font=('Segoe UI',8),anchor='w',wraplength=420).pack(fill='x',pady=(4,0))
        self.jme_compat_frame=tk.Frame(compatibility_panel,bg=self.panel)
        self.jme_compat_frame.pack(fill='both',expand=True)
        tk.Label(self.jme_compat_frame,text='INTERNAL GAME PROFILE',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(5,6))
        self.resolution=ttk.Combobox(self.jme_compat_frame,values=['128 × 160','176 × 208','176 × 220','240 × 320','320 × 240','360 × 640'],state='readonly',width=18)
        self.resolution.set('240 × 320');self.resolution.pack(anchor='w')
        self.resolution.bind('<<ComboboxSelected>>',self.set_resolution)
        tk.Label(self.jme_compat_frame,text='This preserves the original coordinates for game logic.\nThe desktop viewport scales the entire scene to available height.',bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left').pack(anchor='w',pady=(8,16))
        tk.Label(self.jme_compat_frame,text='RENDERER',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(0,6))
        self.renderer=ttk.Combobox(self.jme_compat_frame,values=['GPU · OpenGL','CPU · compatibility'],state='readonly',width=23)
        self.renderer.set('GPU · OpenGL');self.renderer.pack(anchor='w');self.renderer.bind('<<ComboboxSelected>>',self.set_renderer)
        self.execution_compatibility=tk.BooleanVar(value=False)
        ttk.Checkbutton(self.jme_compat_frame,text='Java compatibility mode (slower)',
            variable=self.execution_compatibility,command=self.set_execution_mode).pack(anchor='w',pady=(12,0))
        tk.Label(self.jme_compat_frame,text='For games that stop responding during scene changes. Applies on the next launch.',
            bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w',pady=(4,0))
        self.remove_wrappers=tk.BooleanVar(value=False)
        self.wrapper_option=ttk.Checkbutton(self.jme_compat_frame,text='Skip promotional startup screens (optional)',
            variable=self.remove_wrappers,command=self.set_startup_cleanup)
        self.wrapper_option.pack(anchor='w',pady=(12,0))
        self.startup_info=tk.StringVar(value='Original game startup is used by default.')
        tk.Label(self.jme_compat_frame,textvariable=self.startup_info,bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),
            justify='left',wraplength=420).pack(anchor='w',pady=(5,0))
        self.handling_info=tk.StringVar(value='Game handling analysis appears after selection.')
        tk.Label(self.jme_compat_frame,textvariable=self.handling_info,bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w',pady=(9,0))
        self.nes_compat_frame=tk.Frame(compatibility_panel,bg=self.panel)
        self.nes_runtime_info=tk.StringVar(value='NES runtime status is not known.')
        self.nes_cartridge_info=tk.StringVar(value='Select a cartridge to see its mapper and format.')
        tk.Label(self.nes_compat_frame,text='NES CARTRIDGE',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(5,6))
        tk.Label(self.nes_compat_frame,textvariable=self.nes_cartridge_info,bg=self.panel,fg='#9db7cc',font=('Segoe UI',10),justify='left',wraplength=420).pack(anchor='w',pady=(0,10))
        tk.Label(self.nes_compat_frame,textvariable=self.nes_runtime_info,bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w')
        tk.Label(self.nes_compat_frame,text='Cartridges keep their native 256 × 240 pixels. The aspect choice is saved; this build does not yet apply it in the native player.',bg=self.panel,fg='#7994ac',font=('Segoe UI',9),justify='left',wraplength=420).pack(anchor='w',pady=(12,0))
        self.play_view_note=tk.StringVar(value='All artwork scales together. Width follows height; proportions stay intact.')
        tk.Label(play_panel,textvariable=self.play_view_note,bg=self.panel,fg='#7994ac',font=('Segoe UI',8),anchor='w',wraplength=420).pack(fill='x')
        actions=tk.Frame(play_panel,bg=self.panel);actions.pack(fill='x',pady=(14,10))
        self.play_button=ttk.Button(actions,text='▶  Play enhanced',style='Accent.TButton',command=self.play);self.play_button.pack(side='left',fill='x',expand=True,padx=(0,8))
        self.compare_button=ttk.Button(actions,text='Compare original',command=lambda:self.play(True))
        self.compare_button.pack(side='left')
        self.metrics=tk.StringVar(value='Live performance appears while a game is running.')
        tk.Label(play_panel,textvariable=self.metrics,bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),anchor='w',wraplength=420).pack(fill='x',pady=(0,8))
        tk.Label(art_panel,text='Artwork is prepared in batches when you are not playing. During play, small jobs wait for performance headroom. Saved results are reused.',bg=self.panel,fg='#9db7cc',font=('Segoe UI',9),justify='left',anchor='w',wraplength=420).pack(fill='x',pady=(5,14))
        options=tk.Frame(art_panel,bg=self.panel);options.pack(fill='x',pady=(6,0))
        tk.Label(options,text='ARTWORK STYLE',bg=self.panel,fg='#7994ac',font=('Segoe UI',8,'bold')).pack(anchor='w',pady=(0,5))
        convert=tk.Frame(options,bg=self.panel);convert.pack(fill='x')
        from ai_backend import available
        self.backend=ttk.Combobox(convert,values=['Fast pixel','AI cartoon'],state='readonly',width=12)
        self.backend.set('AI cartoon' if available() else 'Fast pixel');self.backend.pack(side='left',padx=(0,8))
        ttk.Button(convert,text='Prepare',command=self.prepare_style).pack(side='right')
        links=tk.Frame(art_panel,bg=self.panel);links.pack(fill='x',pady=(14,0))
        ttk.Button(links,text='Saved files & logs',command=self.open_folder).pack(side='left',padx=(0,8))
        ttk.Button(links,text='AI setup',command=self.install_ai).pack(side='left')
        self.status = tk.StringVar(value='Choose a game system and add a game to begin.')
        footer=tk.Frame(main,bg=self.bg);footer.pack(side='bottom',before=body,fill='x',pady=(10,0))
        background=tk.Frame(footer,bg=self.bg);background.pack(fill='x',pady=(0,6))
        self.preparation_info=tk.StringVar(value='Artwork cache is ready.')
        tk.Label(background,textvariable=self.preparation_info,bg=self.bg,fg='#7994ac',font=('Segoe UI',9),anchor='w').pack(side='left',fill='x',expand=True)
        self.pause_button=ttk.Button(background,text='Pause background',command=self.toggle_preparation)
        self.pause_button.pack(side='right')
        tk.Label(footer,textvariable=self.status,bg=self.bg,fg='#b7c9d9',font=('Segoe UI',9),anchor='w',wraplength=880).pack(fill='x')
        self.progress=ttk.Progressbar(footer,mode='indeterminate');self.progress.pack(fill='x',pady=(8,10))
        tk.Label(footer,text='CONTROLS  Choose a profile in Play · Edit shared keys in Settings     •     Live artwork waits for stable captures and performance headroom',bg=self.bg,fg='#6e879e',font=('Segoe UI',8),anchor='w').pack(fill='x')
        self._launcher_widget_colors=capture_widget_colors(root)
        if self.visible_system_id==NES:
            self._apply_system_palette()
        self._last_metrics=0
        self._preview_key=None
        self._show_system_details()
        self._refresh_system_presentation()
        self.refresh()
        root.protocol('WM_DELETE_WINDOW', self.close)
        root.after(120, self.poll)
        root.after(1500, self.capture_tick)
        threading.Thread(target=self.detect_gpus,daemon=True).start()
        threading.Thread(target=self.bootstrap_asset_service,daemon=True).start()

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

    def _on_asset_stats(self, game_id, stats):
        try:
            ref=self.library.reference(game_id)
        except (KeyError, LookupError, ValueError):
            return
        self.messages.put(('asset_stats',(ref,stats)))

    def _show_system_details(self):
        is_nes=self.visible_system_id==NES
        if is_nes:
            self.jme_compat_frame.pack_forget()
            self.nes_compat_frame.pack(fill='both',expand=True)
            self.desktop_fit_label.grid_remove()
            self.aspect_combo.grid(row=1,column=1,sticky='ew',padx=(12,0),pady=(5,0))
            self.compare_button.configure(text='Play original')
            self.compare_button.pack(side='left')
            self.play_view_note.set('Enhanced mode captures NES tiles on first play and prepares them after you close the game. Original mode stays available.')
        else:
            self.nes_compat_frame.pack_forget()
            self.jme_compat_frame.pack(fill='both',expand=True)
            self.aspect_combo.grid_remove()
            self.desktop_fit_label.grid(row=1,column=1,sticky='ew',padx=(12,0),pady=(5,0))
            self.compare_button.configure(text='Compare original')
            self.compare_button.pack(side='left')
            self.play_view_note.set('All artwork scales together. Width follows height; proportions stay intact.')

    def _refresh_system_presentation(self):
        label=SYSTEM_LABELS[self.visible_system_id]
        self.collection_title.set(f'Your {label} collection')
        presentation=self.library.presentation(self.visible_system_id)
        demo=presentation.demo
        available=demo is not None and demo.is_file()
        self.demo_button.configure(
            text=f'Import {label} demo',
            state='normal' if available else 'disabled')

    def set_visible_system(self):
        selected=self.system_choice.get()
        if selected not in SYSTEM_LABELS or selected==self.visible_system_id:
            return
        try:
            self.app_settings.save_visible_system(selected)
        except (OSError,ValueError) as error:
            self.system_choice.set(self.visible_system_id)
            self.status.set(f'Could not save the library choice: {error}')
            return
        self.visible_system_id=selected
        self._apply_system_palette()
        self._show_system_details()
        self._refresh_system_presentation()
        self._preview_key=None
        self._show_catalog()
        self.status.set(f'Showing the {SYSTEM_LABELS[selected]} library. Open games and queued work keep running.')

    def bootstrap_asset_service(self):
        def report(error):
            self.messages.put(('background_status', f'Artwork cache recovery skipped: {error}'))
        try:
            self.library.recover_assets(report)
        except (OSError, ValueError, LookupError) as error:
            report(str(error))

    def close(self):
        self._delete_workers.shutdown(wait=False)
        self._preview_worker.shutdown(wait=False, cancel_futures=True)
        self.library.close(timeout=1.0)
        self.root.destroy()

    def detect_gpus(self):
        try:
            found=gpu_devices()
            text='\n'.join(f"{device['kind'].capitalize()}: {device['name']}" for device in found) or 'No Vulkan devices detected. Playback depends on the Windows graphics driver.'
            self.messages.put(('gpu_inventory',text))
        except Exception as error:self.messages.put(('gpu_inventory',f'GPU detection unavailable: {error}'))

    def set_gpu(self,key,label):
        save_gpu_settings(**{key:GPU_CHOICES[label]})
        self.status.set('GPU preference saved. Playback applies on next launch; conversion applies to the next artwork batch.')

    def _refresh_graphics_backend_info(self, preference=None):
        preference=preference or self.graphics_backend_store.load()
        if preference.warning:
            self.graphics_backend_info.set(
                'Renderer settings could not be read; Legacy GPU compatibility is active. '+preference.warning)
        elif preference.backend==LEGACY_BACKEND:
            self.graphics_backend_info.set(
                'Uses the Java ME compatibility renderer. CPU compatibility is still available per game.')
        elif self.graphics_runtime_locator.locate() is None:
            self.graphics_backend_info.set(
                'Shared renderer files are unavailable; launches will continue with Legacy GPU compatibility. Details are saved in the player log.')
        else:
            self.graphics_backend_info.set(
                'Shared renderer is the default. Legacy GPU and per-game CPU compatibility remain available.')

    def set_graphics_backend(self,event=None):
        label=self.graphics_backend_choice.get()
        backend=GRAPHICS_BACKEND_CHOICES.get(label)
        if backend is None:
            preference=self.graphics_backend_store.load()
            self.graphics_backend_choice.set(next(key for key,value in GRAPHICS_BACKEND_CHOICES.items()
                                                   if value==preference.backend))
            return
        try:
            preference=self.graphics_backend_store.save(backend)
        except (OSError,ValueError) as error:
            current=self.graphics_backend_store.load()
            self.graphics_backend_choice.set(next(key for key,value in GRAPHICS_BACKEND_CHOICES.items()
                                                   if value==current.backend))
            self.status.set(f'Could not save the renderer choice: {error}')
            return
        self._refresh_graphics_backend_info(preference)
        if backend==COMMON_BACKEND and self.graphics_runtime_locator.locate() is None:
            self.status.set('Shared GPU selected. This launch will use Legacy GPU compatibility because the shared runtime is unavailable.')
        else:
            self.status.set('Java ME GPU renderer saved. The choice applies on the next launch.')

    def refresh(self):
        self._catalog = self.library.games()
        self._preview_key = None
        self._show_catalog()

    def _schedule_catalog_filter(self, *_):
        if self._catalog_filter_pending is None:
            self._catalog_filter_pending=self.root.after_idle(self._apply_catalog_filter)

    def _apply_catalog_filter(self):
        self._catalog_filter_pending=None
        self._show_catalog()

    def _show_catalog(self):
        selection = self.tree.selection()
        selected = selection[0] if selection else None
        scroll_position=self.tree.yview()[0]
        self.games = {
            game['id']: game for game in self._catalog
            if game['id'] not in self.deleting_games and
            GameRef.from_metadata(game).system == self.visible_system_id
        }
        for ref, stats in self._asset_stats.items():
            if ref.system==self.visible_system_id and ref.id in self.games:
                self.games[ref.id]['asset_work'] = stats
        query=self.search.get().strip().casefold()
        for item in self.tree.get_children():
            self.tree.delete(item)
        for key, game in self.games.items():
            if not query or query in game['name'].casefold():
                self.tree.insert('', 'end', iid=key, text=game['name'], values=('',''))
        visible=self.tree.get_children()
        if selected in visible:
            self.tree.selection_set(selected)
            self.tree.yview_moveto(scroll_position)
        elif visible:
            self.tree.selection_set(visible[0])
        else:
            self.detail_title.set('No games found')
            self.detail_subtitle.set(f'Add a {SYSTEM_LABELS[self.visible_system_id]} game to your collection.')
            self.cache_info.set('No game selected')
            self.compat.set('Saved progress is retained when a game is deleted.')
            self.cover_status.set(f'{SYSTEM_LABELS[self.visible_system_id].upper()} LIBRARY')
            self.cover.configure(image='',text='NES' if self.visible_system_id==NES else 'JAR',width=0)
            self._preview_key=None
            self._preview_request=None
            self.metrics.set('Select a game to play.')
            self.handling_info.set('Game handling analysis appears after selection.')
            self.play_button.configure(state='disabled')
            self.compare_button.configure(state='disabled')
            self.nes_cartridge_info.set('Select a cartridge to see its mapper and format.')
            self.nes_runtime_info.set('The NES player has not been set up.')
        self._refresh_control_profile_controls(self.game_if_selected())
        self._update_row_actions()

    def _update_row_actions(self):
        self.row_actions.set_enabled(not self.busy and not self.pending_imports)
        self.row_actions.schedule()

    def play_row(self,identity):
        if identity not in self.games or not self.tree.exists(identity):return
        game=self.games[identity]
        self.tree.selection_set(identity);self.selected();self.play(game=game)

    def library_double_click(self,event):
        if self.tree.identify_region(event.x,event.y)=='tree':
            identity=self.tree.identify_row(event.y)
            if identity:self.play_row(identity)
        return 'break'

    def delete_row(self,identity):
        if identity not in self.games or not self.tree.exists(identity):return
        game=self.games[identity]
        self.tree.selection_set(identity);self.selected();self.delete_game(game=game)

    def game(self):
        if self.busy:
            self.status.set('Finish the current library operation before changing a game.')
            return None
        selected = self.tree.selection()
        if not selected:
            messagebox.showinfo('Choose a game', 'Select a game in your library first.')
            return None
        return self.games[selected[0]]

    def selected(self, event=None):
        self._update_row_actions()
        selected = self.tree.selection()
        if selected and selected[0] in self.games:
            game = self.games[selected[0]]
            if GameRef.from_metadata(game).system==NES:
                self._show_nes_game(game)
                self._refresh_control_profile_controls(game)
                return
            self._refresh_control_profile_controls(game)
            self.resolution.set(f"{game['width']} × {game['height']}")
            self.quality.set('Detailed · 4×' if game.get('render_scale',4)==4 else 'Balanced · 2×')
            self.renderer.set('CPU · compatibility' if game.get('renderer')=='cpu' else 'GPU · OpenGL')
            self.execution_compatibility.set(game.get('execution_mode','normal') in ('compatible','interpreted'))
            from startup_cleaner import supports_cleanup
            supported=supports_cleanup(game['id'])
            self.remove_wrappers.set(bool(game.get('remove_startup_wrappers',False)))
            self.wrapper_option.configure(state='normal' if supported else 'disabled')
            self.startup_info.set(('Verified MobileRated / HOVR repair available. Original JAR is preserved.'
                if supported else 'No verified wrapper repair for this file. Original startup is retained.')+
                ('\nNext launch skips promotional startup screens.' if supported and self.remove_wrappers.get() else ''))
            self.backend.set('AI cartoon' if game.get('backend')=='ai' else 'Fast pixel')
            self.detail_title.set(game['name'])
            self.detail_subtitle.set(f"{game.get('vendor','Java ME')}  ·  Desktop height · Original proportions")
            inspection=game.get('graphics_inspection',{})
            strategy=inspection.get('handling_plan',{}).get('primary_strategy','unknown')
            labels={'packed_2d_with_custom_decoding':'Packed 2D graphics · decoder/source tracking needed',
                    'mixed_2d_sources_and_decoding':'Mixed 2D images and decoding · tracked sources use HD',
                    'source_images_and_regions':'Source images and sprite regions · preserve sheet coordinates',
                    'source_images':'Source-image replacement',
                    '3d_or_mixed_2d_3d':'3D or mixed rendering · dedicated 3D enhancement needed',
                    'unknown':'Unknown drawing path · runtime tracing needed'}
            decoding=game.get('asset_decoding',{})
            handling=('Common image resources prepared · runtime discovery is enabled for other decoded artwork'
                      if decoding.get('decision')=='standard-images' else
                      'Unknown archive data · runtime discovery will observe artwork after game decoding'
                      if decoding.get('decision') in ('unknown','unresolved-pixel-path') else labels.get(strategy,strategy))
            self.handling_info.set('HANDLING: '+handling+'\nFull gameplay coverage is not verified.')
            partial=inspection.get('packed_graphics_likely',False) or decoding.get('decision') in ('unknown','unresolved-pixel-path')
            self.play_button.configure(text='▶  Play enhanced',state='normal')
            self.compare_button.configure(state='normal')
            self.compat.set(('Coverage is partial · additional artwork may appear after the game decodes it' if partial else
                             'Prepared sources and runtime-discovered artwork share one cache. Gameplay coverage is not yet measured.')+
                            '\nCompatibility: '+game.get('compatibility','not verified for all gameplay'))
            self._request_java_preview(game)
            self._show_java_artwork(game)

    def _request_java_preview(self, game):
        ref=GameRef.from_metadata(game)
        backend=game.get('backend','pixel')
        icon=game.get('manifest',{}).get('MIDlet-Icon','').lstrip('/')
        key=(ref, backend, icon)
        if key==self._preview_key:
            return
        self._preview_key=key
        self._preview_count=None
        self.cover.configure(image='',text='JAR',width=0)
        self._preview_request=(key, not game.get('asset_work'))

    def _poll_java_preview(self):
        # Keep one job in flight and replace the pending request on each selection.
        if self._preview_future is not None:
            request, future=self._preview_future
            if not future.done():
                return
            self._preview_future=None
            game=self.game_if_selected()
            is_current=request is self._preview_request
            if is_current:
                self._preview_request=None
            if (is_current and game is not None
                    and GameRef.from_metadata(game)==request[0][0]):
                try:
                    plate, self._preview_count=future.result()
                except (OSError, ValueError, LookupError):
                    plate=None
                if plate is not None:
                    from PIL import ImageTk
                    self._cover_image=ImageTk.PhotoImage(plate,master=self.root)
                    self.cover.configure(image=self._cover_image,text='',width=0)
                self._show_java_artwork(game)
        if self._preview_request is not None:
            request=self._preview_request
            (ref, backend, icon), count_textures=request
            future=self._preview_worker.submit(
                load_java_preview, self.library, ref, backend, icon, count_textures, self.card)
            self._preview_future=(request,future)

    def _show_java_artwork(self, game):
        work=game.get('asset_work',{})
        waiting=work.get('QUEUED',0)+work.get('DEFERRED',0)
        count=work.get('READY',0) if work else self._preview_count
        if work:
            active=work.get('RUNNING',0)
            failed=work.get('FAILED',0)
            live=work.get('live_reason') if work.get('player_active') else None
            self.cache_info.set(f'{count} enhanced · {waiting} waiting · {active} active'+
                                (f' · {failed} need attention' if failed else ''))
            self.preparation_info.set(f'Artwork: {count} ready · {waiting} waiting · {active} active'+
                (f' · {failed} failed' if failed else '')+(f'\nLive artwork: {live}' if live else '')+
                (f"\n{work['resource_reason']}" if work.get('resource_reason') else ''))
        elif count is None:
            self.cache_info.set('Saved artwork details are loading.' if self._preview_request
                                else 'Saved artwork details are unavailable.')
        else:
            style='AI cartoon' if game.get('backend')=='ai' else 'Fast pixel'
            self.cache_info.set(f'{count} enhanced image'+('s' if count!=1 else '')+f' saved  ·  {style}')
        sources=work.get('sources',game.get('assets',0))
        status=f'{sources} SOURCES' if sources else 'READY TO PLAY'
        if sources and count is not None:
            status+=f' · {count} HD READY'
        self.cover_status.set(status+(f' · {waiting} WAITING' if waiting else ''))

    def _show_nes_game(self,game):
        """Render cartridge facts without reading Java ME-only metadata."""
        self.quality.set('Detailed · 4×' if game.get('render_scale',2)==4 else 'Balanced · 2×')
        self.aspect.set(game.get('aspect','4:3'))
        self.backend.set('AI cartoon' if game.get('backend')=='ai' else 'Fast pixel')
        self.detail_title.set(game['name'])
        self.detail_subtitle.set('Nintendo Entertainment System · Native 256 × 240 · Aspect-preserving desktop view')
        details=game.get('nes',{})
        self.nes_cartridge_info.set(
            f"{details.get('container','unknown').upper()} cartridge\n"
            f"Mapper {details.get('mapper','?')} · Submapper {details.get('submapper','?')}\n"
            f"{str(details.get('region','unknown')).upper()} · {game.get('aspect','4:3')}"
        )
        runtime_status=game.get('runtime_status','setup-needed')
        if runtime_status=='setup-needed':
            self.nes_runtime_info.set('NES player setup is required. Import and saved settings are available; playback stays disabled until a compatible player is installed.')
            self.cover_status.set('CARTRIDGE SAVED · PLAYER MISSING')
            self.play_button.configure(text='NES player setup required',state='disabled')
            self.compare_button.configure(state='disabled')
        elif runtime_status=='unsupported':
            self.nes_runtime_info.set('The NES runtime reports that this cartridge is unsupported.')
            self.cover_status.set('CARTRIDGE UNSUPPORTED')
            self.play_button.configure(text='Cartridge unsupported',state='disabled')
            self.compare_button.configure(state='disabled')
        else:
            self.compare_button.configure(text='Play original',state='normal')
            if self.library.enhanced_available(NES):
                self.nes_runtime_info.set('Managed NES player available. Enhanced capture is opt-in and artwork preparation waits until the game closes.')
                self.cover_status.set('ORIGINAL & ENHANCED AVAILABLE')
                self.play_button.configure(text='▶  Play enhanced',state='normal')
            else:
                self.nes_runtime_info.set('Original playback is available, but this player build cannot capture NES tiles for enhancement.')
                self.cover_status.set('ORIGINAL AVAILABLE · CAPTURE BUILD NEEDED')
                self.play_button.configure(text='Enhanced capture unavailable',state='disabled')
        self._show_nes_artwork(game)
        self.compat.set('Original gameplay remains available. Enhanced mode uses the same mapper and controls, with saved tile replacements when ready. Full-game coverage is not verified.')
        self.handling_info.set('NES tiles are captured during an explicit Enhanced session. The shared artwork worker prepares them after play; results are cached per cartridge.')
        self.metrics.set('NES performance details are not available from this runtime yet.')
        self._preview_key=None
        self._preview_request=None
        self.cover.configure(image='',text='NES',width=0)

    def _show_nes_artwork(self, game):
        work=game.get('asset_work',{})
        if work:
            sources=work.get('sources',0)
            ready=work.get('READY',0)
            waiting=work.get('QUEUED',0)+work.get('DEFERRED',0)+work.get('RUNNING',0)
            failed=work.get('FAILED',0)
            self.cache_info.set(f'{sources} captured tiles · {ready} enhanced · {waiting} waiting'+
                                (f' · {failed} failed' if failed else ''))
        else:
            self.cache_info.set('No NES tiles prepared yet. Enhanced play records visible tiles; close the game to prepare them.')

    def delete_game(self,game=None):
        if self.busy or self.pending_imports:
            self.status.set('Finish the current library operation before deleting a game.')
            return
        if game is None:
            game=self.game()
        if game is None:return
        ref=GameRef.from_metadata(game)
        game_name=game['name']
        if ref.id in self.deleting_games:return
        old=self.players.get(ref.id)
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close this game before deleting it so its saves can finish writing.')
            return
        if not messagebox.askyesno('Delete game?',
                f"Delete {game['name']} from your {SYSTEM_LABELS[ref.system]} collection?\n\n"
                f"Removes the imported {'cartridge' if ref.system==NES else 'JAR'}, artwork, caches and logs.\n"
                'Keeps all saved progress. Import the same game again to reuse it.',parent=self.root):return
        self.deleting_games[ref.id]=game
        self._show_catalog()
        self._update_row_actions()
        self.status.set(f'Deleting {len(self.deleting_games)} game(s) in background. Saved progress is kept.')
        def worker():
            try:
                self.library.delete_game(ref)
                self.messages.put(('deleted',ApplicationEvent('deleted',ref.system,ref.id,game_name)))
            except Exception as error:
                self.messages.put(('delete_error',ApplicationEvent('delete_error',ref.system,ref.id,
                                                                     f'{game_name}\n{error}')))
        self._delete_workers.submit(worker)

    def _refresh_control_profile_controls(self, game=None):
        system=GameRef.from_metadata(game).system if game else self.visible_system_id
        store=self._profile_store_for(system)
        result=store.load()
        self.control_profile_problem=result.problem
        labels={'Default':DEFAULT_SLOT_ID}
        for profile in result.slots:
            labels[f'{profile.slot_id[-1]} · {profile.name}']=profile.slot_id
        self.control_profile_choice_ids=labels
        self.control_profile_combo.configure(values=tuple(labels))
        default_id=DEFAULT_SLOT_ID
        selected_id=game.get('control_profile_id',default_id) if game else default_id
        label=next((label for label,slot_id in labels.items() if slot_id==selected_id),'Default')
        self.control_profile.set(label)
        enabled=bool(game) and not self.busy and game['id'] not in self.deleting_games
        self.control_profile_combo.configure(state='readonly' if enabled else 'disabled')
        if result.problem:
            self.control_profile_info.set(f'Profiles could not be read. Default remains available. {result.problem}')
        elif game and selected_id != DEFAULT_SLOT_ID and label=='Default':
            self.control_profile_info.set('This game’s saved profile is missing. Default controls are selected.')
        elif not result.slots:
            self.control_profile_info.set('Create shared profiles in Settings.')
        else:
            self.control_profile_info.set('Profiles are managed in Settings. Choice applies next launch.')

    def open_settings(self):
        """Open or raise the one global keyboard-settings window."""
        current=self.settings_dialog
        if current is not None and current.top.winfo_exists():
            current.top.deiconify()
            current.top.lift()
            return
        system=self.visible_system_id
        stores={key:self._profile_store_for(key) for key in (JAVA_ME,NES)}
        self.settings_dialog=ControlProfileDialog(
            self.root,stores[system],self._control_profiles_changed,
            schema=NES_CONTROL_SCHEMA if system==NES else JME_CONTROL_SCHEMA,
            color=nes_color if system==NES else java_color,stores=stores)

    def _profile_store_for(self,system):
        if hasattr(self.library,'control_profiles_for'):
            return self.library.control_profiles_for(system)
        return self.nes_control_profiles if system==NES else self.library.control_profiles

    def _control_profiles_changed(self, _slot_id):
        # A shared-slot save changes available labels, never a game's chosen ID.
        self._refresh_control_profile_controls(self.game_if_selected())

    def game_if_selected(self):
        selection=self.tree.selection()
        if selection and selection[0] in self.games:
            return self.games[selection[0]]
        return None

    def set_control_profile(self,event=None):
        game=self.game_if_selected()
        label=self.control_profile.get()
        slot_id=self.control_profile_choice_ids.get(label)
        if (game is None or slot_id is None or self.busy
                or game['id'] in self.deleting_games):
            self._refresh_control_profile_controls(game)
            return
        try:
            self.library.set_control_profile_id(GameRef.from_metadata(game),slot_id)
        except (OSError,RuntimeError,ValueError) as error:
            self._refresh_control_profile_controls(game)
            self.status.set(f'Could not save control profile: {error}')
            return
        if slot_id==DEFAULT_SLOT_ID:
            game.pop('control_profile_id',None)
        else:
            game['control_profile_id']=slot_id
        self._refresh_control_profile_controls(game)
        self.status.set('Control profile preference saved for this game.')

    def set_quality(self,event=None):
        game=self.game()
        if not game:return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing display detail.')
            self.selected();return
        game['render_scale']=4 if self.quality.get().startswith('Detailed') else 2
        self.library.save(game)
        self.status.set('Display detail saved. Existing 4× artwork will be reused; no conversion is required.')

    def set_aspect(self,event=None):
        game=self.game()
        if game is None or GameRef.from_metadata(game).system!=NES:
            return
        game['aspect']=self.aspect.get()
        try:
            self.library.save(game)
        except (OSError,RuntimeError,ValueError) as error:
            self.status.set(f'Could not save NES aspect: {error}')
            self.selected()
            return
        self.status.set('NES aspect preference saved. The current native player still uses its built-in presentation ratio.')

    def set_renderer(self,event=None):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing the renderer.');self.selected();return
        game['renderer']='cpu' if self.renderer.get().startswith('CPU') else 'gpu'
        self.library.save(game)
        self.status.set('Renderer saved. The next launch uses this setting.')

    def set_execution_mode(self):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        player=self.players.get(game['id'])
        if player and player[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing Java compatibility mode.')
            self.selected();return
        game['execution_mode']='compatible' if self.execution_compatibility.get() else 'normal'
        self.library.save(game)
        self.status.set('Java execution setting saved for this game. Applies on the next launch; prepared artwork is reused.')

    def set_startup_cleanup(self):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        game['remove_startup_wrappers']=bool(self.remove_wrappers.get())
        self.library.save(game);self.selected()
        self.status.set('Startup option saved. Applies next time you play; saved artwork is reused.')

    def toggle_preparation(self):
        self.preparation_paused=not self.preparation_paused
        self.pause_button.configure(text='Resume background' if self.preparation_paused else 'Pause background')
        if self.preparation_paused:
            self.library.pause_all()
        else:
            self.library.resume_all()
        self.preparation_info.set('Preparation paused after current images finish.' if self.preparation_paused else 'Preparation resumed.')

    def set_resolution(self, event=None):
        game = self.game()
        if game and GameRef.from_metadata(game).system==JAVA_ME:
            if game['id'] in self.players and self.players[game['id']][0].poll() is None:
                messagebox.showinfo('Game is running', 'Close the game before changing its phone screen size.')
                self.selected()
                return
            game['width'], game['height'] = map(int, self.resolution.get().split(' × '))
            game['screen_source']='manual'
            self.library.save(game)
            self.refresh()

    def import_demo(self):
        presentation = self.library.presentation(self.visible_system_id)
        demo = presentation.demo
        if demo is not None and demo.exists():
            self.import_game(demo)
        elif self.visible_system_id==NES:
            messagebox.showinfo('No NES demo available','No synthetic NES demo is included yet. Choose your own .nes cartridge.')
        else:
            messagebox.showerror('Demo not built', 'Run python tools/build.py to build the player and demo.')

    def import_game(self, path=None):
        if not path:
            presentation = self.library.presentation(self.visible_system_id)
            path = filedialog.askopenfilename(title=presentation.picker_title, filetypes=presentation.filetypes)
        if not path:
            return
        backend = 'ai' if self.backend.get() == 'AI cartoon' else 'pixel'
        request=ImportRequest(self.visible_system_id, path, backend)
        self.pending_imports.append(request)
        self.status.set(f'Queued {os.path.basename(path)}. You can add more games while preparation finishes.')
        self.start_pending()

    def start_pending(self):
        if self.busy:return
        if not self.pending_imports or any(p.poll() is None for p,_ in self.players.values()):return
        request=self.pending_imports.popleft()
        self.busy=True
        self.status.set(f'Preparing {SYSTEM_LABELS[request.system]} · {request.path.name} · {len(self.pending_imports)} more games queued')
        self.progress.start()
        def worker():
            try:
                game = self.library.import_game(request, lambda msg: self.messages.put((
                    'scoped_status',ApplicationEvent('status',request.system,detail=msg))))
                ref=GameRef.from_metadata(game)
                self.messages.put(('imported',ApplicationEvent('imported',ref.system,ref.id,game['name'])))
            except Exception as error:
                self.messages.put(('error',ApplicationEvent('error',request.system,detail=str(error))))
            finally:
                self.messages.put(('idle',ApplicationEvent('idle',request.system)))
        threading.Thread(target=worker, daemon=True).start()

    def install_ai(self):
        if self.busy: return
        self.busy=True
        self.progress.start()
        def worker():
            try:
                from tools.setup_ai import install
                install(lambda msg:self.messages.put(('status',msg)))
            except Exception as error:
                self.messages.put(('error',str(error)))
            finally:
                self.messages.put(('idle',None))
        threading.Thread(target=worker,daemon=True).start()

    def prepare_style(self):
        if self.busy: return
        game=self.game()
        if not game: return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing its artwork style.')
            return
        backend='ai' if self.backend.get()=='AI cartoon' else 'pixel'
        ref=GameRef.from_metadata(game)
        self.busy=True; self.progress.start()
        def worker():
            try:
                game_ready=self.library.prepare_style(game,backend,lambda msg:self.messages.put((
                    'scoped_status',ApplicationEvent('status',ref.system,ref.id,msg))))
                self.messages.put(('imported',ApplicationEvent('imported',ref.system,ref.id,game_ready['name'])))
            except Exception as error:
                self.messages.put(('scoped_error',ApplicationEvent('error',ref.system,ref.id,str(error))))
            finally:
                self.messages.put(('idle',ApplicationEvent('idle',ref.system,ref.id)))
        threading.Thread(target=worker,daemon=True).start()

    def play(self, original=False, game=None):
        if self.busy:
            self.status.set('Finish preparing artwork before starting the player.')
            return
        game = game or self.game()
        if not game:
            return
        ref=GameRef.from_metadata(game)
        capture=False
        if ref.system==NES:
            if game.get('runtime_status')=='setup-needed':
                self.status.set('The NES player is not installed; this cartridge remains saved in your library.')
                return
            if game.get('runtime_status')=='unsupported':
                self.status.set('This NES cartridge is reported as unsupported by the installed player.')
                return
            capture=not original
            if capture and not self.library.enhanced_available(NES):
                self.status.set('This NES player build cannot capture tiles; Original playback is still available.')
                return
        self.launch_game(game,original,capture=capture)

    def launch_game(self,game,original=False,*,capture=False):
        ref=GameRef.from_metadata(game)
        try:
            old = self.players.get(ref.id)
            if old and old[0].poll() is None:
                messagebox.showinfo('Already running', 'This game is already open. Close its player window before launching it again.')
                return
            if ref.system==NES:
                process,log=self.library.launch(game,original,capture=capture)
            else:
                process,log=self.library.launch(game, original)
            self.players[ref.id] = (process,log)
            self.player_refs[ref.id]=ref
            self._notify_player_started(ref,log)
            hint=' '+game.get('startup_hint','')
            if ref.system==NES:
                mode='Original' if original else 'Enhanced'
                suffix='; captured tiles will be prepared after close' if capture else ''
                self.status.set(f"Playing {game['name']} in {mode} NES mode{suffix}."+hint)
            else:
                self.status.set(f"Playing {game['name']}. Small live image jobs need fresh frame and input headroom; larger work waits until play ends."+hint)
        except Exception as error:
            try:self.library.set_player_state(ref,False)
            except KeyError:pass
            if not any(process.poll() is None for process,_ in self.players.values()) and not self.preparation_paused:
                self.library.resume_all()
            messagebox.showerror('Could not start game', str(error))

    def open_folder(self):
        game = self.game()
        if game:
            os.startfile(self.library.workspace(GameRef.from_metadata(game)))

    def capture_tick(self):
        # A player can start before recovery registers its artwork workspace.
        for ref in tuple(self._pending_player_states):
            player=self.players.get(ref.id)
            if player is None or player[0].poll() is not None:
                self._pending_player_states.discard(ref)
            else:
                self._notify_player_started(ref,player[1])
        self.root.after(1500,self.capture_tick)

    def _notify_player_started(self, ref, log):
        try:
            self.library.set_player_state(ref,True,log.with_suffix('.stats.json'),
                                          allow_live=(ref.system!=NES))
        except KeyError:
            self._pending_player_states.add(ref)
        else:
            self._pending_player_states.discard(ref)

    def poll(self):
        # Drain bursts without spending the entire UI tick on progress messages.
        artwork_updates=set()
        for _ in range(100):
            try:
                kind, value = self.messages.get_nowait()
            except queue.Empty:
                break
            if kind == 'status':
                self.status.set(value+(f' · {len(self.pending_imports)} games queued' if self.pending_imports else ''))
            elif kind == 'background_status':
                self.preparation_info.set(value)
            elif kind == 'asset_stats':
                ref,stats=value
                if not isinstance(ref,GameRef):continue
                self._asset_stats[ref]=stats
                if ref.system==self.visible_system_id and ref.id in self.games:
                    self.games[ref.id]['asset_work']=stats
                artwork_updates.add(ref)
            elif kind == 'gpu_inventory':
                self.gpu_inventory.set(value)
            elif kind == 'imported':
                self._handle_imported_event(value)
            elif kind == 'deleted':
                self._handle_deleted_event(value)
            elif kind == 'delete_error':
                self._handle_delete_error_event(value)
            elif kind == 'idle':
                self.busy = False
                self.progress.stop()
                self.refresh()
            elif kind == 'error':
                if isinstance(value,ApplicationEvent):
                    label=SYSTEM_LABELS[value.system]
                    self.status.set(f'{label} preparation failed. Your original file has not been changed.')
                    messagebox.showerror(f'{label} preparation failed',value.detail)
                else:
                    self.status.set('Preparation failed. Your original file has not been changed.')
                    messagebox.showerror('Preparation failed', value)
            elif kind == 'scoped_error':
                label=SYSTEM_LABELS[value.system]
                self.status.set(f'{label} operation failed. Your original file was not changed.')
                messagebox.showerror(f'{label} operation failed',value.detail)
            elif kind == 'scoped_status':
                self.status.set(f"{SYSTEM_LABELS[value.system]}: {value.detail}")
            elif kind == 'refresh':
                self.refresh()
        game=self.game_if_selected()
        if game is not None and (ref:=GameRef.from_metadata(game)) in artwork_updates:
            if ref.system==NES:
                self._show_nes_artwork(game)
            else:
                self._show_java_artwork(game)
        self._poll_java_preview()
        if time.monotonic()-self._last_metrics>1:
            self._last_metrics=time.monotonic()
            self.update_telemetry()
        for key, (process, log) in list(self.players.items()):
            result = process.poll()
            if result is not None:
                del self.players[key]
                ref=self.player_refs.pop(key,None)
                self._pending_player_states.discard(ref)
                try:self.library.set_player_state(ref or key,False)
                except KeyError:pass
                if result:
                    reason = ('The GPU player could not continue. Try CPU · compatibility in the Compatibility tab, or check your graphics driver.'
                              if result == 71 and ref is not None and ref.system==JAVA_ME else
                              'The NES player could not continue; check its saved runtime log and setup.'
                              if result == 71 and ref is not None and ref.system==NES else
                              'The game could not continue. Its diagnostic log may show a compatibility error.'
                              if result == 72 and ref is not None and ref.system==JAVA_ME else
                              f'The player exited with code {result}.')
                    messagebox.showerror('Player stopped', f'{reason}\n\nDetails: {log}')
        self.start_pending()
        self.row_actions.set_enabled(not self.busy and not self.pending_imports)
        self.root.after(120, self.poll)

    def _handle_imported_event(self,event):
        ref=event.ref
        if ref.system==self.visible_system_id:
            self.search.set('')
            self.refresh()
            if self.tree.exists(ref.id):
                self.tree.selection_set(ref.id)
                self.tree.see(ref.id)
            self.status.set(f"{event.detail} is ready in the {SYSTEM_LABELS[ref.system]} library.")
        else:
            self.status.set(f"{event.detail} was added to the {SYSTEM_LABELS[ref.system]} library. The open screen was left unchanged.")

    def _handle_deleted_event(self,event):
        ref=event.ref
        self._catalog=[game for game in self._catalog if GameRef.from_metadata(game)!=ref]
        self.deleting_games.pop(ref.id,None)
        self._asset_stats.pop(ref,None)
        self.players.pop(ref.id,None)
        self.player_refs.pop(ref.id,None)
        if ref.system==self.visible_system_id:
            self._show_catalog()
        self.status.set(f"{event.detail} deleted from the {SYSTEM_LABELS[ref.system]} library. Saved progress kept."+
            (f' {len(self.deleting_games)} deletion(s) pending.' if self.deleting_games else ''))

    def _handle_delete_error_event(self,event):
        ref=event.ref
        self.deleting_games.pop(ref.id,None)
        if ref.system==self.visible_system_id:
            self._show_catalog()
        title=event.detail.splitlines()[0]
        self.status.set(f"{title} deletion failed; recovery files have been retained. Saved progress is kept.")
        messagebox.showerror('Could not delete game',event.detail,parent=self.root)

    def update_telemetry(self):
        selection=self.tree.selection()
        if not selection:return
        key=selection[0]
        game=self.games.get(key)
        if game and GameRef.from_metadata(game).system==NES:
            self.metrics.set('NES frame and input measurements are not connected to the library screen yet.')
            return
        active=self.players.get(key)
        if not active or active[0].poll() is not None:
            self.metrics.set('Saved artwork is reused. Temporary resized sprites do not need AI conversion.')
            return
        _,log=active
        try:
            report=json.loads(Path(str(log.with_suffix('.stats.json'))+'.gpu.json').read_text(encoding='utf-8'))
            suffix='' if report.get('matched',True) else '\nWindows selected a different GPU from the requested preference.'
            self.active_gpu.set('Actual playback: '+report['device']+suffix)
        except (OSError,ValueError,KeyError):pass
        try:
            metrics=json.loads(log.with_suffix('.stats.json').read_text(encoding='utf-8'))
            if time.time()-metrics['updated']<4:
                live=self.games.get(key,{}).get('asset_work',{}).get('live_reason')
                scaler=metrics.get('fast_scaler_submit_ms',0.0)
                actual_backend=metrics.get('backend','Player')
                requested_backend=metrics.get('requested_backend')
                if actual_backend=='common-v1':
                    backend_label='Shared GPU · common-v1'
                elif actual_backend=='legacy-java' and requested_backend=='common-v1':
                    backend_label='Legacy GPU fallback · see player log'
                elif actual_backend=='legacy-java':
                    backend_label='Legacy GPU · compatibility'
                elif actual_backend=='Java2D CPU':
                    backend_label='CPU · compatibility'
                else:
                    backend_label=actual_backend
                detail=f"{backend_label}  ·  {metrics['fps']:.1f} game FPS\nInput queue p95 {metrics['input_queue_p95_ms']:.1f} ms  ·  {metrics['scale']}× detail, desktop fit"
                if scaler:detail+=f'\nGPU scaler CPU submission {scaler:.2f} ms (last texture)'
                if metrics.get('gpu_scale_fallbacks',0):
                    detail+='\nGPU filter fallback is in use; details are in the player log.'
                elif metrics.get('gpu_scale_passes',0):
                    detail+='\nLive GPU filter active; detailed artwork prepares in the background.'
                self.metrics.set(detail+(f'\nLive artwork: {live}' if live else ''))
            else:self.metrics.set('Waiting for screen updates; menus may redraw only after input.')
        except (OSError,ValueError,KeyError):
            self.metrics.set('Player starting… waiting for frame measurements.')
        try:
            with log.open('rb') as file:
                file.seek(0,2);end=file.tell();file.seek(max(0,end-8192));text=file.read().decode('utf-8',errors='replace')
            if 'Exception in thread' in text or 'MIDlet event failed' in text:
                self.compat.set('Compatibility: an error was detected — see saved logs')
        except OSError:pass


def main():
    # A second launcher could otherwise keep converting on the same GPU during play.
    import msvcrt
    lock_path=DATA_ROOT/'library/.launcher.lock'
    lock_path.parent.mkdir(parents=True,exist_ok=True)
    with lock_path.open('a+b') as lock:
        lock.seek(0,2)
        if not lock.tell():lock.write(b'1');lock.flush()
        lock.seek(0)
        try:msvcrt.locking(lock.fileno(),msvcrt.LK_NBLCK,1)
        except OSError:
            notice=tk.Tk();notice.withdraw()
            messagebox.showinfo('JAR Revival is already open','Use the existing JAR Revival window. Only one library runs at a time to avoid competing conversion jobs.')
            notice.destroy();return
        root=tk.Tk()
        App(root)
        root.mainloop()


if __name__ == '__main__':
    main()
