export type InvitationDetails = {
	workspaceId: string;
	role: 'admin' | 'editor' | 'viewer';
	accepted: boolean;
	expiresAt: string;
};

async function request<T>(path: string, method = 'GET'): Promise<T> {
	const response = await fetch(path, {
		method,
		credentials: 'same-origin',
		cache: 'no-store',
		headers: method === 'POST' ? { 'Content-Type': 'application/json' } : {},
		...(method === 'POST' ? { body: '{}' } : {}),
	});
	if (!response.ok) {
		if (response.status === 401)
			throw new Error('Sign in before accepting this invitation.');
		if (response.status === 409)
			throw new Error(
				'This invitation was already accepted by another account.',
			);
		throw new Error('This invitation is expired, revoked, or unavailable.');
	}
	return response.json() as Promise<T>;
}

export function loadInvitation(token: string): Promise<InvitationDetails> {
	return request(`/api/invitations/${encodeURIComponent(token)}`);
}

export function acceptWorkspaceInvitation(
	token: string,
): Promise<{ workspaceId: string; role: InvitationDetails['role'] }> {
	return request(
		`/api/invitations/${encodeURIComponent(token)}/accept`,
		'POST',
	);
}
