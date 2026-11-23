"""Prepare Java ME artwork, previews, and discovery reports."""
from __future__ import annotations

import json
import os
import re
import shutil
from concurrent.futures import ThreadPoolExecutor, wait, FIRST_COMPLETED
from PIL import Image
from runtime_assets.pixel import enhance
from storage.atomic import atomic_json
from runtime_assets.contracts import REDRAW_VERSION, image_source_id as image_id

RECIPE = 'scale2x-rgba-v1-4x'
RECIPE_LABELS = {'pixel': RECIPE, 'ai': 'realesrgan-anime-alpha-v1-4x', 'redraw': REDRAW_VERSION}
MAX_PIXELS = 4_194_304
_ASSET_KEY = re.compile(r"[0-9a-f]{64}\Z")


class JavaMeArtwork:
    def recipe_for_backend(self, backend):
        return self._artwork_recipes.for_backend(backend)

    def texture_dir(self, folder, backend):
        return folder / {"ai": "textures-ai", "redraw": "textures-redraw"}.get(backend, "textures")

    def prepare_ui_assets(self, folder):
        """Smooth bitmap text without asking a neural model to redraw individual letters."""
        catalogue=json.loads((folder/'catalogue.json').read_text(encoding='utf-8'))
        for item in catalogue:
            image_id = item.get('image_id')
            if not isinstance(image_id, str) or _ASSET_KEY.fullmatch(image_id) is None:
                continue
            resource=item['resource'].replace('\\','/').lower()
            if 'font' not in resource:continue
            source=folder/'original-assets'/(image_id+'.png')
            target=folder/'textures-ui'/source.name
            if target.exists():continue
            target.parent.mkdir(parents=True,exist_ok=True)
            with Image.open(source) as image:
                result=image.convert('RGBA').resize((image.width*4,image.height*4),Image.Resampling.LANCZOS)
                result.save(target)

    def prepare_asset(self, folder, key, image, backend='pixel'):
        if not isinstance(key, str) or _ASSET_KEY.fullmatch(key) is None:
            raise ValueError('Artwork key must be a 64-character lowercase hash.')
        target = self.texture_dir(folder, backend) / (key + '.png')
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            return False
        ui=folder/'textures-ui'/(key+'.png')
        if ui.exists():
            shutil.copy2(ui,target);return True
        if not (folder / 'original-assets' / (key + '.png')).exists():
            image.save(folder / 'original-assets' / (key + '.png'))
        if backend == 'redraw':
            from runtime_assets.sprite_paint import redraw_asset
            enhanced = redraw_asset(image)
        else:
            enhanced = enhance(image)
        if backend == 'ai':
            from runtime_assets.ai import upscale
            enhanced = upscale(image, folder, enhanced.getchannel('A'))
        temp = target.with_suffix('.tmp')
        enhanced.save(temp, format='PNG')
        os.replace(temp, target)
        return True

    def prepare_many(self, folder, assets, backend, workers, progress=lambda msg: None, stop_event=None):
        """Bound both running work and decoded images; cached entries are never resubmitted."""
        jobs = [(key,path) for key,path in assets if not (self.texture_dir(folder,backend)/(key+'.png')).exists()]
        workers = max(1,min(8,int(workers)))
        if not jobs:
            progress('All artwork is already cached — no conversion needed.')
            return 0
        progress(f'Enhancing {len(jobs)} images with {workers} parallel workers ({backend})…')
        def convert(job):
            key,path=job
            with Image.open(path) as decoded:
                if decoded.width*decoded.height>MAX_PIXELS:
                    raise ValueError('Image exceeds the supported dimensions')
                image=decoded.convert('RGBA')
            return self.prepare_asset(folder,key,image,backend)
        count=0
        iterator=iter(jobs)
        with ThreadPoolExecutor(max_workers=workers,thread_name_prefix='asset') as pool:
            pending=set()
            for _ in range(workers):
                if stop_event is not None and stop_event.is_set():break
                job=next(iterator,None)
                if job is not None:pending.add(pool.submit(convert,job))
            completed=0
            while pending:
                ready,pending=wait(pending,return_when=FIRST_COMPLETED)
                for future in ready:
                    count+=bool(future.result())
                    completed+=1
                    progress(f'Enhanced {completed}/{len(jobs)} images • up to {workers} in parallel')
                    job=next(iterator,None) if stop_event is None or not stop_event.is_set() else None
                    if job is not None: pending.add(pool.submit(convert,job))
        return count

    def prepare_style(self, game, backend, progress=lambda msg: None):
        folder = self.workspace(game['id'])
        with self.lock:
            self._check_not_deleting(game['id'])
            fresh_sources = {}
            self.decode_assets(game, progress, fresh_sources=fresh_sources)
            self.prepare_ui_assets(folder)
            self.record_decoding_coverage(game)
            current=json.loads((folder/'game.json').read_text(encoding='utf-8'))
            migration_game = dict(current)
            migration_game['asset_decoding'] = game.get('asset_decoding', {})
            current.pop('packed_decoder',None)
            for key in ('decoded_assets', 'decoded_scope', 'decoded_packs_reached',
                        'decoded_packs_total', 'runtime_assets'):
                current.pop(key, None)
            current.update(backend=backend, recipe=RECIPE_LABELS[backend])
            for key in ('assets','asset_decoding'):
                if key in game:current[key]=game[key]
            recipe, ai_recipe = self.recipe_for_backend(backend)
            current['recipe_key'] = recipe.key
            if ai_recipe is not None:
                current['model_digest'] = recipe.model_digest
            else:
                current.pop('model_digest', None)
            self.save(current)
            self.asset_service.register_game(folder, recipe, game=migration_game,
                                             ai_recipe=ai_recipe, progress=progress,
                                             fresh_sources=fresh_sources)
            current['asset_work'] = self.asset_service.stats(game['id'])
            self.save(current)
            progress('Artwork work was queued. The game is ready to play while conversion continues.')
            return current

    def decode_assets(self, game, progress=lambda msg: None, *, fresh_sources=None):
        from systems.jme.resources.decoding import decode_jar
        folder=self.workspace(game['id'])
        report,catalogue=decode_jar(self.library/game['id']/'game.jar',folder,image_id,progress=progress,
                                    fresh_sources=fresh_sources)
        game['asset_decoding']=report
        game['assets']=report['unique_images']
        return report,catalogue

    def enhance_sources(self, game, backend, sources, progress=lambda msg: None):
        folder=self.workspace(game['id'])
        return self.prepare_many(folder,sources,backend,game.get('workers',self.workers),progress)

    def record_decoding_coverage(self, game):
        folder=self.workspace(game['id'])
        inspection=folder/'graphics-inspection.json'
        if inspection.is_file():
            report=json.loads(inspection.read_text(encoding='utf-8'))
            report.setdefault('handling_plan',{})['source_decoding']=game.get('asset_decoding',{})
            report['handling_plan']['complete_hd_coverage']=None
            report['coverage_note']=game.get('asset_decoding',{}).get('coverage',
                'runtime coverage is not yet measured')
            atomic_json(inspection,report)
