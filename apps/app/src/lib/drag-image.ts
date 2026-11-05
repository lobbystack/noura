/**
 * The name tag that follows a row dragged out of the file tree, drawn as a
 * PNG for the native drag so it looks like the tag shown inside the app.
 */
export async function dragLabelImage(
	label: string,
): Promise<{ image: number[]; imageScale: number } | null> {
	try {
		const scale = Math.min(Math.max(Math.round(devicePixelRatio || 1), 1), 4);
		// Read the theme's colors and font from an element styled like the tag.
		const probe = document.createElement('div');
		probe.className =
			'bg-popover text-popover-foreground border border-border text-xs';
		probe.style.position = 'fixed';
		probe.style.visibility = 'hidden';
		document.body.append(probe);
		const style = getComputedStyle(probe);
		const colors = {
			background: style.backgroundColor,
			text: style.color,
			border: style.borderTopColor,
		};
		const font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
		probe.remove();

		const canvas = document.createElement('canvas');
		const context = canvas.getContext('2d');
		if (!context) return null;
		context.font = font;
		const text = fitted(context, label, 176);
		const width = Math.ceil(context.measureText(text).width) + 16;
		const height = 24;
		canvas.width = width * scale;
		canvas.height = height * scale;
		context.scale(scale, scale);
		context.beginPath();
		context.roundRect(0.5, 0.5, width - 1, height - 1, 8);
		context.fillStyle = colors.background;
		context.fill();
		context.strokeStyle = colors.border;
		context.stroke();
		context.font = font;
		context.fillStyle = colors.text;
		context.textBaseline = 'middle';
		context.fillText(text, 8, height / 2 + 0.5);

		const blob = await new Promise<Blob | null>((resolve) =>
			canvas.toBlob(resolve, 'image/png'),
		);
		if (!blob) return null;
		const image = Array.from(new Uint8Array(await blob.arrayBuffer()));
		return { image, imageScale: scale };
	} catch {
		return null;
	}
}

/** `text`, shortened with an ellipsis to fit `maxWidth`. */
function fitted(
	context: CanvasRenderingContext2D,
	text: string,
	maxWidth: number,
): string {
	if (context.measureText(text).width <= maxWidth) return text;
	let end = text.length;
	while (
		end > 1 &&
		context.measureText(`${text.slice(0, end)}…`).width > maxWidth
	)
		end -= 1;
	return `${text.slice(0, end)}…`;
}
