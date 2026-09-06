/** Optional real S3-compatible endpoint for the integration suite. Never uses production variables. */
export function testS3Client(): Bun.S3Client | undefined {
	const endpoint = process.env.NOURA_TEST_S3_ENDPOINT;
	if (!endpoint) return undefined;
	const required = (name: string) => {
		const value = process.env[name];
		if (!value) throw new Error(`${name} is required for S3 integration tests`);
		return value;
	};
	return new Bun.S3Client({
		endpoint,
		bucket: required('NOURA_TEST_S3_BUCKET'),
		accessKeyId: required('NOURA_TEST_S3_ACCESS_KEY'),
		secretAccessKey: required('NOURA_TEST_S3_SECRET_KEY'),
		region: 'us-east-1',
	});
}
