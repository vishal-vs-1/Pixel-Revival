import io
import json
import lzma
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile
from PIL import Image

from systems.jme.resources.decoding import DecoderRegistry, decode_jar, default_registry, embedded_pngs
from runtime_assets.contracts import image_source_id as image_id
from systems.jme.playback import runtime_arguments
from systems.jme.resources.gameloft import layout, unpack
from systems.jme.resources.indexed import IndexedImages


class AssetDecodingTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)

    def png(self,color='red'):
        buffer=io.BytesIO();Image.new('RGBA',(7,9),color).save(buffer,format='PNG')
        return buffer.getvalue()

    def jar(self,entries):
        path=self.root/'game.jar'
        with zipfile.ZipFile(path,'w') as archive:
            for name,data in entries.items():archive.writestr(name,data)
        return path

    def decode(self,path,folder='out'):
        return decode_jar(path,self.root/folder,image_id,'unused-java',self.root/'missing-player.jar')

    def pack(self,blocks):
        header=6+4*(len(blocks)+1);offsets=[header]
        for block in blocks:offsets.append(offsets[-1]+len(block))
        return struct.pack('<HHH',len(blocks),1,0)+struct.pack('<'+'I'*len(offsets),*offsets)+b''.join(blocks)

    def test_ordinary_images_skip_custom_code_and_repeat_uses_cache(self):
        path=self.jar({'texture.png':self.png(),'main.class':b'not executed'})
        with patch('subprocess.run',side_effect=AssertionError('Image import must not execute code')):
            report,records=self.decode(path)
            stamp=(self.root/'out/original-assets'/(records[0]['image_id']+'.png')).stat().st_mtime_ns
            again,_=self.decode(path)
        self.assertFalse(report['additional_decoding'])
        self.assertEqual(report['unique_images'],1)
        self.assertEqual(report,again)
        self.assertEqual(stamp,(self.root/'out/original-assets'/(records[0]['image_id']+'.png')).stat().st_mtime_ns)

    def test_static_lzma_container_images_and_unknown_codec_are_distinguished(self):
        container=self.pack([bytes([129])+lzma.compress(self.png(),format=lzma.FORMAT_ALONE),
                             bytes([2])+b'\xdf\x05unknown-sprite-codec'])
        path=self.jar({'asset.pack':container})
        with patch('subprocess.run',side_effect=AssertionError('Unknown code must not execute')):
            report,records=self.decode(path)
        self.assertTrue(report['additional_decoding'])
        self.assertEqual(report['container_blocks'],2)
        self.assertEqual(report['unique_images'],1)
        self.assertEqual(report['features']['gameloft_sprites']['unsupported_blocks'], 1)
        self.assertNotIn('decoded_rgb_lookup',report['features'])

    def indexed_sprite(self, *payloads, encoding=0x56f2):
        header = struct.pack('<HIH', 0x05df, 0x01000000, len(payloads))
        modules = b'\x00\x02\x01' * len(payloads)
        frame_tables = struct.pack('<4H', 0, 0, 0, 0)
        palettes = struct.pack('<HBB4I', 0x8888, 2, 2, 0x00112233, 0xff445566, 0, 0x804488cc)
        pixels = struct.pack('<H', encoding) + b''.join(
            struct.pack('<H', len(payload)) + payload for payload in payloads)
        return header + modules + frame_tables + palettes + pixels

    def test_packed_sprite_images_use_runtime_ids_and_keep_palette_provenance(self):
        payload = self.indexed_sprite(b'\x82\x00\x01')
        container = self.pack([bytes([129]) + lzma.compress(payload, format=lzma.FORMAT_ALONE)])
        path = self.jar({'art.pack': container})
        with patch('subprocess.Popen', side_effect=AssertionError('Import must not launch a process')):
            report, records = self.decode(path)
            cached, _ = self.decode(path)
        self.assertEqual(report, cached)
        self.assertEqual(report['outcomes']['gameloft-indexed-sprites-v1'], 2)
        self.assertEqual(report['features']['gameloft_sprites']['decoded_blocks'], 1)
        self.assertEqual([(row['resource'], row['module'], row['palette_bank'], row['palette']) for row in records],
                         [('art.pack#0', 0, 0, 0), ('art.pack#0', 0, 0, 1)])
        expected = [((17, 34, 51, 0), (68, 85, 102, 255)), ((0, 0, 0, 0), (68, 136, 204, 128))]
        for row, pixels in zip(records, expected):
            image = Image.new('RGBA', (2, 1))
            image.putdata(pixels)
            self.assertEqual(image_id(image), row['image_id'])

    def test_sprite_blocks_fail_closed_and_respect_import_limits(self):
        valid = self.indexed_sprite(b'\x82\x00\x01')
        bad_index = self.indexed_sprite(b'\x82\x00\x01', b'\x82\x00\x02')
        path = self.jar({'art.pack': self.pack([b'\x02' + bad_index, b'\x02' + valid[:-1],
                                               b'\x02' + valid])})
        report, records = self.decode(path)
        self.assertEqual(report['features']['gameloft_sprites']['unsupported_blocks'], 2)
        self.assertEqual([row['resource'] for row in records], ['art.pack#2'] * 2)
        with patch('systems.jme.resources.gameloft_artwork._MAX_IMPORT_IMAGES', 1):
            limited, records = self.decode(path, 'limited')
        self.assertEqual(limited['features']['gameloft_sprites']['limited_blocks'], 2)
        self.assertEqual(limited['features']['gameloft_sprites']['unsupported_blocks'], 1)
        self.assertEqual(records, [])

    def test_packed_sprite_alpha_runs_override_palette_alpha(self):
        alpha = b'\xfe\x02\x80'
        container = self.pack([b'\x02' + self.indexed_sprite(color + alpha, encoding=encoding)
                               for encoding, color in ((0xa640, b'\x03'), (0xa127, b'\x82\x01'))])
        report, records = self.decode(self.jar({'art.pack': container}))
        self.assertEqual(report['features']['gameloft_sprites']['decoded_blocks'], 2)
        expected = {image_id(Image.new('RGBA', (2, 1), color)) for color in
                    ((68, 85, 102, 128), (68, 136, 204, 128))}
        self.assertEqual({row['image_id'] for row in records}, expected)
        self.assertEqual(len(records), 4)

    def test_raw_index_sprites_in_new_container_types_are_decoded(self):
        # Wrath of the Titans stores one palette-index byte per pixel inside
        # content-type 3 blocks. The old decoder only looked at type 2 and did
        # not know 0x5602, so it extracted none of these sprites.
        payload = self.indexed_sprite(b'\x00\x01', encoding=0x5602)
        report, records = self.decode(self.jar({'art.pack': self.pack([bytes([3]) + payload])}))
        self.assertEqual(report['outcomes']['gameloft-indexed-sprites-v1'], 2)
        self.assertEqual(report['features']['gameloft_sprites']['decoded_blocks'], 1)
        expected = [((17, 34, 51, 0), (68, 85, 102, 255)),
                    ((0, 0, 0, 0), (68, 136, 204, 128))]
        for row, pixels in zip(records, expected):
            image = Image.new('RGBA', (2, 1))
            image.putdata(pixels)
            self.assertEqual(image_id(image), row['image_id'])

    def test_truncated_tables_and_decompression_limit_are_rejected(self):
        payload=self.pack([bytes([129])+lzma.compress(b'A'*100,format=lzma.FORMAT_ALONE)])
        self.assertIsNone(layout('pack',{'pack':payload[:-1]}.__getitem__))
        parts=layout('pack',{'pack':payload}.__getitem__)
        with patch('systems.jme.resources.gameloft.MAX_BLOCK',20),self.assertRaises(ValueError):
            list(unpack(parts,{'pack':payload}.__getitem__))

    def test_embedded_png_crc_and_unsupported_format(self):
        png=self.png();corrupt=bytearray(png);corrupt[-1]^=1
        self.assertEqual(len(list(embedded_pngs(b'prefix'+png+b'suffix'))),1)
        self.assertEqual(list(embedded_pngs(corrupt)),[])
        report,_=self.decode(self.jar({'resource':b'unknown graphics data'}))
        self.assertEqual(report['decision'],'unknown')

    def test_png_header_is_restored_only_when_original_crcs_validate(self):
        png = self.png()
        swapped = bytearray(png)
        for index in range(0, 40, 2):
            swapped[index:index+2] = png[index:index+2][::-1]
        corrupt = swapped.copy()
        corrupt[-1] ^= 1
        path = self.jar({'data.bin': swapped, 'corrupt.bin': corrupt})
        report, records = self.decode(path)
        self.assertEqual(report['unclassified_resources'], ['corrupt.bin'])
        self.assertTrue(report['additional_decoding'])
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]['decoder'], 'swapped-png-header-v1')
        self.assertEqual(records[0]['image_id'], image_id(Image.new('RGBA', (7, 9), 'red')))

    def test_raw_indexed_frames_use_declared_palette_and_cache_key(self):
        path = self.jar({'frames.bin': b'\x00\x01\x01\x00', 'palette': b'\x01\x02\x03\x04\x05\x06'})
        spec = dict(resource='frames.bin', palette='palette', width=2, height=1, frames=2,
                    transparent_index=0)

        def decode(specification):
            indexed = IndexedImages(json.dumps([specification]).encode())
            registry = DecoderRegistry((*default_registry().decoders, indexed))
            return decode_jar(path, self.root/'out', image_id, registry=registry)

        report, records = decode(spec)
        self.assertEqual(report['outcomes']['indexed-rgb8-v1'], 2)
        self.assertEqual(report['unclassified_resources'], [])
        self.assertEqual([row['frame'] for row in records], [0, 1])
        with Image.open(self.root/'out/original-assets'/(records[0]['image_id']+'.png')) as image:
            self.assertEqual(image.getpixel((0, 0)), (1, 2, 3, 0))
            self.assertEqual(image.getpixel((1, 0)), (4, 5, 6, 255))
        _, changed = decode(dict(spec, transparent_index=1))
        self.assertNotEqual(records[0]['image_id'], changed[0]['image_id'])
        with self.assertRaisesRegex(ValueError, 'payload length'):
            decode(dict(spec, frames=3))

    def test_runtime_observation_policy_is_shared_and_adds_no_control_workarounds(self):
        self.assertEqual(runtime_arguments('0'*64),[])
        game={'asset_decoding':{'features':{'decoded_rgb_lookup':True}}}
        self.assertEqual(runtime_arguments('0'*64,game),[])
        self.assertEqual([decoder.name for decoder in default_registry().decoders],
                         ['standard-images-v1','embedded-png-v1','gameloft-offset-lzma-v1',
                          'swapped-png-header-v1','gameloft-indexed-sprites-v1'])

    def test_icon_does_not_hide_an_unknown_packed_or_generated_pixel_path(self):
        path=self.jar({'icon.png':self.png(),'sprite.dat':b'undocumented binary format',
                       'game.class':b'createRGBImage'})
        report,_=self.decode(path)
        self.assertEqual(report['decision'],'unresolved-pixel-path')
        self.assertIsNone(report['additional_decoding'])


if __name__=='__main__':unittest.main()
