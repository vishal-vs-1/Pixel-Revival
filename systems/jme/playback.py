"""Launch Java ME players with controls, render options, and character artwork."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import time
from uuid import uuid4
from systems.comparison import ComparisonPlayers, comparison_startup_options, copy_playback_files
from systems.jme.profiles import PROFILES
from systems.jme.execution_policy import java_execution_arguments
from storage.paths import RESOURCE_ROOT
from systems.jme.pose_artwork import PoseArtwork
from controls.java_me import CUSTOM_SLOT_IDS, resolve_selection, serialize_bindings
from systems.jme.jdk import locate_java

ROOT = RESOURCE_ROOT


def runtime_arguments(game_id, game=None):
    """Apply compatibility flags and select this game's versioned asset recipe."""
    compat=PROFILES.get(game_id,{}).get('runtime_compat',{});args=[]
    if compat.get('repaint_on_show'):args.append('-Djme.compat.repaintOnShow=on')
    if compat.get('restore_movement_canvases'):
        args.append('-Djme.input.restoreCanvases='+','.join(compat['restore_movement_canvases']))
    if compat.get('restore_movement_state'):
        args.append('-Djme.input.restoreState='+compat['restore_movement_state'])
    recipe_key=(game or {}).get('recipe_key')
    if isinstance(recipe_key,str) and len(recipe_key)==64:
        args.append(f'-Djme.recipeKey={recipe_key}')
    return args


class JavaMePlayback:
    def regenerate_playable_poses(self, game_id):
        with self.lock:
            self._check_not_deleting(game_id)
            folder = self.workspace(game_id)
            if not (folder / 'game.json').is_file():
                raise FileNotFoundError('This game is no longer in the library.')
            player = self._players.get(game_id)
            if player is not None and player.poll() is None:
                raise RuntimeError('Close the game before regenerating its player poses.')
            return PoseArtwork(folder).regenerate()

    def launch(self, game, original=False, proof=None, *, stop_seconds=0, hidden=False,
               pose_mode=None, comparison=False):
        with self.lock:
            self._check_not_deleting(game['id'])
            if comparison:
                existing = self._players.get(game['id'])
                if existing is not None and existing.poll() is None:
                    raise RuntimeError('Close the current player before comparing this game.')
                folder = self.workspace(game['id'])
                root = folder / '.runtime' / 'compare' / uuid4().hex
                for label in ('original', 'enhanced'):
                    saves = root / label
                    saves.mkdir(parents=True)
                    copy_playback_files(folder / 'saves', saves)
                pair = ComparisonPlayers(lambda original: self._launch(
                    dict(game), original, pose_mode=None if original else (pose_mode or 'record'),
                    comparison_saves=root / ('original' if original else 'enhanced')),
                    ready_paths=(root / 'original.ready', root / 'enhanced.ready'))
                self._players[game['id']] = pair
                return pair, pair.logs[-1]
            return self._launch(game,original,proof,stop_seconds=stop_seconds,hidden=hidden,pose_mode=pose_mode)

    def _launch(self, game, original=False, proof=None, *, stop_seconds=0, hidden=False,
                pose_mode=None, comparison_saves=None):
        if game.get('status') != 'Ready':
            raise RuntimeError('This import has not finished. Import the JAR again to resume.')
        emulator = ROOT / 'build' / 'jme-player.jar'
        if not emulator.exists():
            raise RuntimeError('Player is not built. Run python tools/build.py first.')
        folder = self.workspace(game['id'])
        playable = self.prepare_startup(game)
        log_path = folder / 'logs' / f'play-{time.time_ns()}.log'
        scale = max(1, min(4, 900 // game['height']))
        render_scale=max(2,min(4,int(game.get('render_scale',4))))
        from graphics.gpu_policy import settings,player_java,expected_device
        gpu_mode=settings()['render_gpu']
        java=player_java(gpu_mode) if game.get('renderer','gpu')=='gpu' else locate_java()
        expected=expected_device(gpu_mode) if game.get('renderer','gpu')=='gpu' else None
        selection=game.get('control_profile_id')
        control_resolution=resolve_selection(selection,self.control_profiles.load())
        control_profile_arg=(f'-Djme.controlBindings={serialize_bindings(control_resolution.profile)}'
                             if control_resolution.profile is not None else None)
        args = [java, *java_execution_arguments(game), '-Xmx768m', '-Dfile.encoding=ISO_8859_1', f'-Djme.pack={folder}',
                f'-Djme.stats={log_path.with_suffix(".stats.json")}',
                f"-Djme.textures={self.texture_dir(folder,game.get('backend','pixel'))}",
                f'-Djme.scale={1 if original else render_scale}',
                f'-Djme.captureArtwork={str(comparison_saves is None or not original).lower()}']
        args += [f'-Djme.gpuPreference={gpu_mode}',f'-Djme.expectedGpu={expected["name"] if expected else ""}']
        args += runtime_arguments(game['id'],game)
        if pose_mode in ('record', 'record-replace', 'replace'):
            args.extend(PoseArtwork(folder).launch_arguments(pose_mode != 'replace'))
            args.append(f'-Djme.replacePoses={str(pose_mode != "record").lower()}')
        elif pose_mode == 'regenerated':
            args.extend(PoseArtwork(folder).launch_arguments(True, regenerated=True))
        if control_profile_arg is not None:
            args.append(control_profile_arg)
        launch_options = {}
        if comparison_saves is not None:
            ready = comparison_saves.with_suffix('.ready')
            args.append(f'-Djme.comparisonReady={ready}')
            launch_options = comparison_startup_options()
        if game.get('renderer','gpu')=='gpu':
            if not list((ROOT/'tools/gpu').glob('lwjgl-*.jar')):
                raise RuntimeError('GPU runtime is missing. Run python tools/setup_gpu.py first.')
            title = game['name']
            if comparison_saves is not None:
                title += ' — Original' if original else ' — Enhanced'
            args += ['--enable-native-access=ALL-UNNAMED','-Djme.graphicsBackend=legacy','-Djme.gpu=true','-cp',str(emulator)+os.pathsep+str(ROOT/'tools/gpu/*'),
                     'org.recompile.mobile.GpuPlayer',str(playable),str(game['width']),str(game['height']),title]
        else:
            args += ['-jar',str(emulator),playable.resolve().as_uri(),'1',str(game['width']),str(game['height']),str(scale)]
        if proof:
            args.insert(1, f'-Djme.proof={Path(proof).resolve()}')
        if stop_seconds:args.insert(1,f'-Djme.stopSeconds={int(stop_seconds)}')
        if hidden:args.insert(1,'-Djme.hidden=true')
        working_directory = Path(comparison_saves or folder / 'saves')
        working_directory.mkdir(parents=True, exist_ok=True)
        with log_path.open('w', encoding='utf-8') as log:
            log.write('Startup: '+str(playable)+'\n')
            log.write(game['startup']['note']+'\n')
            if control_resolution.profile is not None:
                log.write('Control profile: '+control_resolution.profile.slot_id+'\n')
            elif control_resolution.reason:
                requested=selection if selection in CUSTOM_SLOT_IDS else 'invalid'
                log.write('Control profile fallback: '+requested+' -> default ('+control_resolution.reason+')\n')
            log.flush()
            process = subprocess.Popen(args, cwd=working_directory,
                                       stdout=log, stderr=subprocess.STDOUT, **launch_options)
        if comparison_saves is None:
            self._players[game['id']]=process
        return process, log_path
