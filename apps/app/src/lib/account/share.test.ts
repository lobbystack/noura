import { describe, expect, test } from 'bun:test';
import {
	createCipheriv,
	generateKeyPairSync,
	randomBytes,
	sign,
} from 'node:crypto';
import { JSDOM } from 'jsdom';
import { decryptShare, shareKeys } from './share';
import { shareMarkdown } from './share-markdown';
import type { WindowLike } from 'dompurify';

const { privateKey, publicKey } = generateKeyPairSync('ed25519');
const key = randomBytes(32);
const signer = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
const fragment = `#key=${key.toString('base64url')}&signer=${signer.toString('base64url')}`;
const payload = {
	version: 1 as const,
	title: 'Private title',
	markdown: '# A shared item\n\nOnly this item.',
	updatedAt: '2026-09-05T00:00:00Z',
};
function fixture(content: unknown = payload) {
	const nonce = randomBytes(12);
	const route = [1, 'workspace', 'object', 'device', 'operation', 1, '0'];
	const cipher = createCipheriv('aes-256-gcm', key, nonce);
	cipher.setAAD(Buffer.from(JSON.stringify(['noura.sync.payload', ...route])));
	const ciphertext = Buffer.concat([
		cipher.update(JSON.stringify(content)),
		cipher.final(),
		cipher.getAuthTag(),
	]).toString('base64');
	const op = {
		version: 1,
		workspaceId: 'workspace',
		objectId: 'object',
		deviceId: 'device',
		operationId: 'operation',
		epoch: 1,
		policyRevision: '0',
		nonce: nonce.toString('base64'),
		ciphertext,
		signature: '',
	};
	op.signature = sign(
		null,
		Buffer.from(
			JSON.stringify(['noura.sync.operation', ...route, op.nonce, ciphertext]),
		),
		privateKey,
	).toString('base64');
	return op;
}
describe('encrypted public shares', () => {
	test('decrypts a signed fixture produced independently with node crypto', async () => {
		expect(await decryptShare(fixture(), shareKeys(fragment))).toEqual(payload);
	});
	test('rejects tampered routing, signatures, wrong keys, and extra plaintext fields', async () => {
		const valid = fixture();
		for (const tampered of [
			{ ...valid, objectId: 'another' },
			{ ...valid, policyRevision: '1' },
			{ ...valid, signature: Buffer.alloc(64).toString('base64') },
			{ ...valid, title: 'leaked' },
			fixture({ ...payload, otherObjects: [] }),
		])
			await expect(
				decryptShare(tampered, shareKeys(fragment)),
			).rejects.toThrow();
		const wrong = shareKeys(fragment);
		wrong.key[0] ^= 1;
		await expect(decryptShare(valid, wrong)).rejects.toThrow();
	});
	test('requires complete, exact, canonical fragment keys', () => {
		for (const bad of [
			'',
			'#key=x',
			fragment + '&key=x',
			fragment + '&extra=x',
			fragment.replace('key=', 'other='),
		])
			expect(() => shareKeys(bad)).toThrow();
	});
	test('renders text without raw HTML, images, unsafe URLs, or automatic resources', () => {
		const window = new JSDOM('').window;
		const render = shareMarkdown(window as unknown as WindowLike);
		const html = render(
			'# Hello\n\n**Safe** [click](https://example.com) [bad](javascript:alert%281%29) ![tracking](https://evil.example/pixel)\n\n<img src="https://evil.example/pixel" onerror="alert(1)"><iframe src="https://evil.example"></iframe>\n\n<svg><a href="javascript:alert(1)">evil</a></svg>',
		);
		const document = new JSDOM(html).window.document;
		expect(document.querySelector('h1')?.textContent).toBe('Hello');
		expect(document.querySelector('strong')?.textContent).toBe('Safe');
		expect(
			document.querySelector('img,svg,iframe,script,style,object,input'),
		).toBeNull();
		expect(document.querySelector('[src],[onerror],[style]')).toBeNull();
		const links = [...document.querySelectorAll('a[href]')];
		expect(links.length).toBe(1);
		expect(links[0]?.getAttribute('rel')).toBe('noopener noreferrer');
		expect(links[0]?.getAttribute('target')).toBe('_blank');
		window.close();
	});
});
