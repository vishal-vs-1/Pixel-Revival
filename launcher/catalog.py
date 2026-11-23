"""Filter the game library and present selected game details."""

from tkinter import messagebox
from launcher.preview import load_java_preview
from systems.contracts import GameRef, NES
from systems.nes.metadata import DEFAULT_RENDER_SCALE
from launcher.labels import SYSTEM_LABELS, ARTWORK_LABELS


class LauncherCatalog:
    def _show_system_details(self):
        is_nes=self.visible_system_id==NES
        if is_nes:
            self.jme_compat_frame.pack_forget()
            self.nes_compat_frame.pack(fill='both',expand=True)
            self.desktop_fit_label.grid_remove()
            self.aspect_combo.grid(row=1,column=1,sticky='ew',padx=(12,0),pady=(5,0))
            self.play_view_note.set('Enhance play uses cached artwork and records tiles and player poses for processing after close.')
        else:
            self.nes_compat_frame.pack_forget()
            self.jme_compat_frame.pack(fill='both',expand=True)
            self.aspect_combo.grid_remove()
            self.desktop_fit_label.grid(row=1,column=1,sticky='ew',padx=(12,0),pady=(5,0))
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
            self.compare_play_button.configure(state='disabled')
            self.enhance_checkbox.configure(state='disabled')
            self.nes_cartridge_info.set('Select a cartridge to see its mapper and format.')
            self.nes_runtime_info.set('The NES player has not been set up.')
        self._refresh_control_profile_controls(self.game_if_selected())
        self.playable_artwork.refresh(self.game_if_selected())
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

    def game_if_selected(self):
        selection=self.tree.selection()
        if selection and selection[0] in self.games:
            return self.games[selection[0]]
        return None

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
            from systems.jme.startup_cleaner import supports_cleanup
            supported=supports_cleanup(game['id'])
            self.remove_wrappers.set(bool(game.get('remove_startup_wrappers',False)))
            self.wrapper_option.configure(state='normal' if supported else 'disabled')
            self.startup_info.set(('Verified MobileRated / HOVR repair available. Original JAR is preserved.'
                if supported else 'No verified wrapper repair for this file. Original startup is retained.')+
                ('\nNext launch skips promotional startup screens.' if supported and self.remove_wrappers.get() else ''))
            self.backend.set(ARTWORK_LABELS.get(game.get('backend'), ARTWORK_LABELS['pixel']))
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
            self.play_button.configure(text='Play',state='normal')
            self.compare_play_button.configure(state='normal')
            self.enhance_checkbox.configure(state='normal')
            self.compat.set(('Coverage is partial · additional artwork may appear after the game decodes it' if partial else
                             'Prepared sources and runtime-discovered artwork share one cache. Gameplay coverage is not yet measured.')+
                            '\nCompatibility: '+game.get('compatibility','not verified for all gameplay'))
            self.playable_artwork.refresh(game)
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
            on_demand=work.get('on_demand',0)
            deferred_note=f' · {on_demand:,} on demand' if on_demand else ''
            live=work.get('live_reason') if work.get('player_active') else None
            self.cache_info.set(f'{count} enhanced · {waiting} waiting · {active} active'+
                                (f' · {failed} need attention' if failed else '')+deferred_note)
            self.preparation_info.set(f'Artwork: {count} ready · {waiting} waiting · {active} active'+
                (f' · {failed} failed' if failed else '')+(f'\nLive artwork: {live}' if live else '')+
                (f"\n{work['resource_reason']}" if work.get('resource_reason') else ''))
        elif count is None:
            self.cache_info.set('Saved artwork details are loading.' if self._preview_request
                                else 'Saved artwork details are unavailable.')
        else:
            style=ARTWORK_LABELS.get(game.get('backend'), ARTWORK_LABELS['pixel'])
            self.cache_info.set(f'{count} enhanced image'+('s' if count!=1 else '')+f' saved  ·  {style}')
        sources=work.get('sources',game.get('assets',0))
        status=f'{sources} SOURCES' if sources else 'READY TO PLAY'
        if sources and count is not None:
            status+=f' · {count} HD READY'
        self.cover_status.set(status+(f' · {waiting} WAITING' if waiting else ''))

    def _show_nes_game(self,game):
        """Render cartridge facts without reading Java ME-only metadata."""
        self.playable_artwork.refresh(game)
        self.quality.set('Detailed · 4×' if game.get('render_scale',DEFAULT_RENDER_SCALE)==4 else 'Balanced · 2×')
        self.aspect.set(game.get('aspect','4:3'))
        self.backend.set(ARTWORK_LABELS.get(game.get('backend'), ARTWORK_LABELS['pixel']))
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
            self.nes_runtime_info.set('The Rust NES player is not built. Import and saved settings are available; playback stays disabled until rust-nes/mesen-player is built.')
            self.cover_status.set('CARTRIDGE SAVED · PLAYER NOT BUILT')
            self.play_button.configure(text='Build the Rust NES player',state='disabled')
            self.compare_play_button.configure(state='disabled')
            self.enhance_checkbox.configure(state='disabled')
        elif runtime_status=='unsupported':
            self.nes_runtime_info.set('The NES runtime reports that this cartridge is unsupported.')
            self.cover_status.set('CARTRIDGE UNSUPPORTED')
            self.play_button.configure(text='Cartridge unsupported',state='disabled')
            self.compare_play_button.configure(state='disabled')
            self.enhance_checkbox.configure(state='disabled')
        else:
            if self.library.enhanced_available(NES):
                self.enhance_checkbox.configure(state='normal')
                self.nes_runtime_info.set('NES plays in this window on the Rust core. Enhanced uses the prepared pack; new tiles are recorded while you play Enhanced.')
                self.cover_status.set('ORIGINAL & ENHANCED AVAILABLE')
                self.play_button.configure(text='Play',state='normal')
                self.compare_play_button.configure(state='normal')
            else:
                self.nes_runtime_info.set('Original playback is available, but this Rust player build cannot capture NES tiles for enhancement.')
                self.cover_status.set('ORIGINAL AVAILABLE · CAPTURE BUILD NEEDED')
                self.enhance_checkbox.configure(state='disabled')
                self.play_button.configure(text='Play',state='normal')
                self.compare_play_button.configure(state='disabled')
        self._show_nes_artwork(game)
        self.compat.set('Original gameplay remains available. Enhanced mode uses the same mapper and controls, with saved tile replacements when ready. Full-game coverage is not verified.')
        self.handling_info.set('NES plays and composites Enhanced frames in this window. Tiles seen during Enhanced play prepare replacements for later launches; results are cached per cartridge.')
        self.metrics.set('NES performance details are not available from this runtime yet.')
        self._preview_key=None
        self._preview_request=None
        self.cover.configure(image='',text='NES',width=0)

    def _show_nes_artwork(self, game):
        work=game.get('asset_work',{})
        summary=None
        if not work:
            try:
                summary=self.library.asset_summary(game)
            except (OSError, RuntimeError, ValueError, KeyError):
                summary=None
        if work or (summary and (summary['sources'] or summary['pack_tiles'])):
            sources=(work.get('sources',0) if work else summary['sources'])
            ready=(work.get('READY',0) if work else summary['ready'])
            waiting=((work.get('QUEUED',0)+work.get('DEFERRED',0)+work.get('RUNNING',0))
                     if work else summary['waiting'])
            failed=(work.get('FAILED',0) if work else summary['failed'])
            pack=('' if not summary or not summary['pack_tiles'] else
                  f" · {summary['pack_tiles']} in {summary['pack_scale']}x pack")
            text=(f'{sources} captured tiles · {ready} enhanced · {waiting} waiting'+
                  (f' · {failed} failed' if failed else ''))
            self.cache_info.set(text+pack)
            self.preparation_info.set(f'Artwork: {sources} captured · {ready} enhanced · {waiting} waiting'+
                                      (f' · {failed} failed' if failed else '')+pack)
        else:
            self.cache_info.set('No NES tiles prepared yet. Enhanced play uses replacements once artwork is prepared.')
            self.preparation_info.set('Artwork: no NES tiles prepared yet.')
