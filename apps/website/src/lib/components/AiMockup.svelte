<script lang="ts">
	import Icon from './Icon.svelte';

	const proposed = [
		'Book the launch venue',
		'Send invites to beta users',
		'Write the follow-up email',
	] as const;
</script>

<div
	class="chat"
	role="img"
	aria-label="An AI chat in Noura. The user shares two notes as context, and the assistant proposes three tasks that wait for approval."
>
	<div class="chat-head">
		<Icon name="sparkle" size={16} />
		<span>AI chat</span>
		<small>Your provider</small>
	</div>

	<div class="context">
		<span class="context-label">Context</span>
		<span class="chip"><Icon name="file" size={13} />Launch brief</span>
		<span class="chip"><Icon name="file" size={13} />Event plan</span>
	</div>

	<div class="bubble user">
		Turn the open questions in the event plan into tasks for the launch project.
	</div>

	<div class="bubble ai">
		I found three open questions. Here are the tasks I'd create in
		<strong>Website relaunch</strong>:
	</div>

	<div class="approval">
		<div class="approval-head">
			<Icon name="lock" size={15} />
			<span>Create 3 tasks</span>
			<small>Needs approval</small>
		</div>
		<ul>
			{#each proposed as task (task)}
				<li><i></i>{task}</li>
			{/each}
		</ul>
		<div class="actions">
			<span class="decline">Decline</span>
			<span class="approve"><Icon name="check" size={14} />Approve</span>
		</div>
	</div>
</div>

<style>
	.chat {
		width: 100%;
		max-width: 520px;
		padding: 20px;
		border: 1px solid #2c2b31;
		border-radius: 20px;
		background: #161618;
		color: #ecebef;
		box-shadow: 0 40px 90px rgba(35, 28, 70, 0.28);
		font-size: 14px;
	}
	.chat-head {
		display: flex;
		align-items: center;
		gap: 8px;
		padding-bottom: 16px;
		border-bottom: 1px solid #28272c;
		color: #b9b8ff;
		font-weight: 700;
	}
	.chat-head span {
		color: #ecebef;
	}
	.chat-head small {
		margin-left: auto;
		padding: 3px 9px;
		border-radius: 999px;
		background: #232227;
		color: #96959c;
		font-size: 12px;
		font-weight: 600;
	}
	.context {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 8px;
		margin: 16px 0 18px;
	}
	.context-label {
		color: #7a7980;
		font-size: 12px;
		font-weight: 600;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 5px 10px;
		border: 1px solid rgba(147, 145, 254, 0.35);
		border-radius: 999px;
		background: rgba(147, 145, 254, 0.1);
		color: #d7d6ff;
		font-size: 12.5px;
	}
	.bubble {
		max-width: 88%;
		margin-bottom: 12px;
		padding: 12px 15px;
		border-radius: 14px;
		line-height: 1.55;
	}
	.user {
		margin-left: auto;
		background: #2a2930;
		border-bottom-right-radius: 5px;
	}
	.ai {
		background: transparent;
		padding-left: 0;
		color: #c9c8ce;
	}
	.ai strong {
		color: #f3f2f6;
	}
	.approval {
		border: 1px solid #34333a;
		border-radius: 14px;
		background: #1d1d21;
		overflow: hidden;
	}
	.approval-head {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 12px 14px;
		border-bottom: 1px solid #2c2b31;
		font-weight: 700;
	}
	.approval-head small {
		margin-left: auto;
		color: #e7a86b;
		font-size: 12px;
		font-weight: 600;
	}
	ul {
		margin: 0;
		padding: 6px 14px;
		list-style: none;
	}
	li {
		display: flex;
		align-items: center;
		gap: 10px;
		height: 36px;
		color: #c9c8ce;
	}
	li i {
		width: 14px;
		height: 14px;
		border: 1.5px solid #5c5b62;
		border-radius: 4px;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		padding: 12px 14px;
		border-top: 1px solid #2c2b31;
	}
	.actions span {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 32px;
		padding: 0 14px;
		border-radius: 999px;
		font-size: 13px;
		font-weight: 700;
	}
	.decline {
		background: #28272c;
		color: #c9c8ce;
	}
	.approve {
		background: #9391fe;
		color: #111114;
	}
</style>
