import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release', Path(__file__).parents[1] / 'scripts/publish-chat-release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class API:
    def __init__(self, login='owner', kind='User', tag=None, existing=None):
        self.login, self.kind, self.tag, self.existing = login, kind, tag, existing
        self.calls = []
        self.assets = list((existing or {}).get("assets", []))
        self.upload_digest = True
        self.extra_after_upload = False

    def request(self, method, path, data=None, **kwargs):
        self.calls.append((method, path, data))
        if path == '/user': return {'login': self.login, 'type': self.kind}
        if '/git/ref/' in path: return self.tag
        if '/releases/tags/' in path: return self.existing
        if method == 'GET' and '/assets?' in path: return list(self.assets)
        if '/git/tags/' in path: return {'object': {'type':'commit','sha':'a'*40}}
        if method == 'POST' and path.endswith('/releases'):
            return {'id': 123, 'draft': True, 'html_url': 'https://github.com/owner/repo/releases/tag/v0.1.67'}
        if 'uploads.github.com' in path:
            asset = {'name':'ctmcp-0.1.67-win64.exe','size':len(data),'digest':'sha256:'+hashlib.sha256(data).hexdigest()}
            if not self.upload_digest: del asset['digest']
            self.assets.append(asset)
            if self.extra_after_upload: self.assets.append({'name':'unexpected.zip'})
            return asset
        if method == 'PATCH': return {'html_url':'https://github.com/owner/repo/releases/tag/v0.1.67'}
        raise AssertionError((method, path))


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root=Path(self.directory.name)
        self.binary=b'MZ'+bytes(2_000_000)
        (self.root/'ctmcp-0.1.67-win64.exe').write_bytes(self.binary)
        (self.root/'SHA256SUMS.txt').write_text(hashlib.sha256(self.binary).hexdigest()+'  ctmcp-0.1.67-win64.exe\n')

    def publish(self, api):
        return release.publish(api,self.root,'owner/repo','owner','0.1.67','a'*40)

    def test_wrong_identity_never_writes(self):
        for api in [API('someone'),API('owner','Bot')]:
            with self.assertRaisesRegex(RuntimeError,'Personal authorization'): self.publish(api)
            self.assertTrue(all(method=='GET' for method,_,_ in api.calls))

    def test_corrupt_binary_never_writes(self):
        (self.root/'ctmcp-0.1.67-win64.exe').write_bytes(b'MZ'+bytes(2_000_001))
        api=API()
        with self.assertRaisesRegex(RuntimeError,'checksum'):self.publish(api)
        self.assertTrue(all(method=='GET' for method,_,_ in api.calls))

    def test_existing_tag_cannot_move(self):
        api=API(tag={'object':{'type':'commit','sha':'b'*40}})
        with self.assertRaisesRegex(RuntimeError,'different commit'): self.publish(api)
        self.assertTrue(all(method=='GET' for method,_,_ in api.calls))

    def test_publish_only_after_single_exe_verified(self):
        api=API(); self.assertTrue(self.publish(api).endswith('v0.1.67'))
        writes=[call for call in api.calls if call[0]!='GET']
        self.assertEqual([call[0] for call in writes],['POST','POST','PATCH'])
        self.assertTrue(writes[0][2]['draft']);self.assertEqual(writes[-1][2],{'draft':False})
        self.assertEqual(writes[1][2],self.binary)
        self.assertIn(hashlib.sha256(self.binary).hexdigest(), writes[0][2]['body'])
        self.assertIn('name=ctmcp-0.1.67-win64.exe', writes[1][1])
        self.assertEqual(len(api.assets),1)

    def test_existing_asset_is_not_overwritten(self):
        api=API(existing={'id':123,'target_commitish':'a'*40,'author':{'login':'owner'},'assets':[{'name':'ctmcp-0.1.67-win64.exe','digest':'sha256:wrong'}]})
        with self.assertRaisesRegex(RuntimeError,'Existing asset'):self.publish(api)
        self.assertTrue(all(method=='GET' for method,_,_ in api.calls))

    def test_unexpected_assets_never_deleted_or_published(self):
        api=API(existing={'id':123,'draft':True,'target_commitish':'a'*40,'author':{'login':'owner'},'assets':[{'name':'old.zip'}]})
        with self.assertRaisesRegex(RuntimeError,'Unexpected existing'): self.publish(api)
        self.assertTrue(all(method=='GET' for method,_,_ in api.calls))

    def test_retry_reuses_verified_exe_without_duplicate_upload(self):
        asset={'name':'ctmcp-0.1.67-win64.exe','size':len(self.binary),'digest':'sha256:'+hashlib.sha256(self.binary).hexdigest()}
        existing={'id':123,'draft':True,'target_commitish':'a'*40,'author':{'login':'owner'},'assets':[asset]}
        api=API(existing=existing);self.publish(api)
        self.assertEqual([c[0] for c in api.calls if c[0]!='GET'],['PATCH'])
        existing.update(draft=False,html_url='https://github.com/owner/repo/releases/tag/v0.1.67')
        api=API(existing=existing);self.publish(api)
        self.assertTrue(all(c[0]=='GET' for c in api.calls))

    def test_missing_upload_digest_leaves_draft(self):
        api=API();api.upload_digest=False
        with self.assertRaisesRegex(RuntimeError,'Uploaded asset failed'): self.publish(api)
        self.assertFalse(any(c[0]=='PATCH' for c in api.calls))

    def test_final_inventory_detects_extra_asset(self):
        api=API();api.extra_after_upload=True
        with self.assertRaisesRegex(RuntimeError,'exactly one'): self.publish(api)
        self.assertFalse(any(c[0]=='PATCH' for c in api.calls))

    def test_existing_draft_target_and_author_are_guarded(self):
        for fields in [{'target_commitish':'b'*40,'author':{'login':'owner'}}, {'target_commitish':'a'*40,'author':{'login':'other'}}]:
            api=API(existing={'id':123,'draft':True,**fields})
            with self.assertRaises(RuntimeError): self.publish(api)
            self.assertTrue(all(c[0]=='GET' for c in api.calls))

    def test_annotated_tag_can_resolve_to_tested_commit(self):
        api=API(tag={'object':{'type':'tag','sha':'c'*40}})
        self.publish(api)
        self.assertTrue(any('/git/tags/' in c[1] for c in api.calls))


if __name__ == '__main__': unittest.main()
