<script lang="ts">
	import type { Attachment } from 'svelte/attachments';

	const motionSpeed = 0.75;

	const vertexSource = `#version 300 es
    precision highp float;

    void main() {
      vec2 positions[6] = vec2[6](
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0)
      );
      gl_Position = vec4(positions[gl_VertexID], 0.0, 1.0);
    }
  `;

	const fragmentSource = `#version 300 es
    precision highp float;

    uniform vec2 u_resolution;
    uniform vec2 u_pointer;
    uniform float u_pointer_active;
    uniform float u_time;

    out vec4 fragColor;

    float hash(vec2 point) {
      vec3 p = fract(vec3(point.xyx) * 0.1031);
      p += dot(p, p.yzx + 33.33);
      return fract((p.x + p.y) * p.z);
    }

    float noise(vec2 point) {
      vec2 cell = floor(point);
      vec2 local = fract(point);
      local = local * local * (3.0 - 2.0 * local);

      return mix(
        mix(hash(cell), hash(cell + vec2(1.0, 0.0)), local.x),
        mix(hash(cell + vec2(0.0, 1.0)), hash(cell + vec2(1.0)), local.x),
        local.y
      );
    }

    float fbm(vec2 point) {
      float value = 0.0;
      float weight = 0.55;
      mat2 turn = mat2(0.82, 0.57, -0.57, 0.82);

      for (int octave = 0; octave < 5; octave++) {
        value += weight * noise(point);
        point = turn * point * 2.03 + 7.13;
        weight *= 0.48;
      }

      return value;
    }

    float field(vec2 point, float time) {
      vec2 firstWarp = vec2(
        fbm(point + vec2(0.0, time * 0.08)),
        fbm(point + vec2(5.2, 1.3) - time * 0.06)
      );
      vec2 secondWarp = vec2(
        fbm(point + 2.4 * firstWarp + vec2(1.7, 9.2) + time * 0.035),
        fbm(point + 2.1 * firstWarp + vec2(8.3, 2.8) - time * 0.03)
      );

      return fbm(point + 2.75 * secondWarp);
    }

    void main() {
      vec2 uv = gl_FragCoord.xy / u_resolution;
      float aspect = u_resolution.x / u_resolution.y;
      vec2 point = vec2((uv.x - 0.5) * aspect, uv.y - 0.14);

      vec2 pointer = vec2((u_pointer.x - 0.5) * aspect, u_pointer.y - 0.14);
      vec2 pull = pointer - point;
      float pointerWeight = exp(-dot(pull, pull) * 3.2) * u_pointer_active;
      point += pull * pointerWeight * 0.13;

      float shade = field(point * 1.42 + vec2(0.0, u_time * 0.018), u_time);
      shade = smoothstep(0.22, 0.94, shade);

      float lowerBloom = smoothstep(0.0, 0.95, uv.y);
      float centreBloom = 1.0 - smoothstep(0.0, 0.85, length(point * vec2(0.72, 0.5)));
      shade = clamp(shade * 0.82 + lowerBloom * 0.16 + centreBloom * 0.08, 0.0, 1.0);

      vec3 ink = vec3(0.058, 0.050, 0.066);
      vec3 plum = vec3(0.129, 0.102, 0.157);
      vec3 violet = vec3(0.196, 0.165, 0.263);
      vec3 color = mix(ink, plum, smoothstep(0.05, 0.62, shade));
      color = mix(color, violet, smoothstep(0.58, 1.0, shade) * 0.72);

      float grain = hash(gl_FragCoord.xy + floor(u_time * 12.0)) - 0.5;
      color += grain / 255.0;

      fragColor = vec4(color, 1.0);
    }
  `;

	function compileShader(
		context: WebGL2RenderingContext,
		type: number,
		source: string,
	): WebGLShader | null {
		const shader = context.createShader(type);
		if (!shader) return null;

		context.shaderSource(shader, source);
		context.compileShader(shader);
		if (context.getShaderParameter(shader, context.COMPILE_STATUS))
			return shader;

		context.deleteShader(shader);
		return null;
	}

	function createProgram(context: WebGL2RenderingContext): WebGLProgram | null {
		const vertex = compileShader(context, context.VERTEX_SHADER, vertexSource);
		const fragment = compileShader(
			context,
			context.FRAGMENT_SHADER,
			fragmentSource,
		);
		if (!vertex || !fragment) return null;

		const program = context.createProgram();
		if (!program) return null;

		context.attachShader(program, vertex);
		context.attachShader(program, fragment);
		context.linkProgram(program);
		context.deleteShader(vertex);
		context.deleteShader(fragment);

		if (context.getProgramParameter(program, context.LINK_STATUS))
			return program;

		context.deleteProgram(program);
		return null;
	}

	const mountShader: Attachment<HTMLCanvasElement> = (canvas) => {
		const context = canvas.getContext('webgl2', {
			alpha: false,
			antialias: false,
			depth: false,
			powerPreference: 'low-power',
			stencil: false,
		});
		if (!context) return;

		const program = createProgram(context);
		const vertexArray = context.createVertexArray();
		if (!program || !vertexArray) return;

		const resolutionLocation = context.getUniformLocation(
			program,
			'u_resolution',
		);
		const pointerLocation = context.getUniformLocation(program, 'u_pointer');
		const pointerActiveLocation = context.getUniformLocation(
			program,
			'u_pointer_active',
		);
		const timeLocation = context.getUniformLocation(program, 'u_time');
		const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

		let animationFrame = 0;
		let isVisible = true;
		let pointerActive = 0;
		let pointerTarget = 0;
		let pointerX = 0.5;
		let pointerY = 0.55;
		let smoothPointerX = pointerX;
		let smoothPointerY = pointerY;
		const startedAt = performance.now();

		const resize = () => {
			const bounds = canvas.getBoundingClientRect();
			const scale = window.innerWidth < 700 ? 0.12 : 0.18;
			const width = Math.max(1, Math.round(bounds.width * scale));
			const height = Math.max(1, Math.round(bounds.height * scale));

			if (canvas.width !== width || canvas.height !== height) {
				canvas.width = width;
				canvas.height = height;
				context.viewport(0, 0, width, height);
			}
		};

		const draw = (now: number) => {
			resize();
			smoothPointerX += (pointerX - smoothPointerX) * 0.035;
			smoothPointerY += (pointerY - smoothPointerY) * 0.035;
			pointerActive += (pointerTarget - pointerActive) * 0.045;

			context.useProgram(program);
			context.bindVertexArray(vertexArray);
			context.uniform2f(resolutionLocation, canvas.width, canvas.height);
			context.uniform2f(pointerLocation, smoothPointerX, smoothPointerY);
			context.uniform1f(pointerActiveLocation, pointerActive);
			context.uniform1f(timeLocation, ((now - startedAt) / 1000) * motionSpeed);
			context.drawArrays(context.TRIANGLES, 0, 6);
		};

		const animate = (now: number) => {
			draw(now);
			animationFrame = requestAnimationFrame(animate);
		};

		const stop = () => {
			cancelAnimationFrame(animationFrame);
			animationFrame = 0;
		};

		const updateAnimation = () => {
			stop();
			const shouldAnimate =
				isVisible &&
				document.visibilityState === 'visible' &&
				!reducedMotion.matches;

			if (shouldAnimate) animationFrame = requestAnimationFrame(animate);
			else draw(startedAt);
		};

		const handlePointerMove = (event: PointerEvent) => {
			const bounds = canvas.getBoundingClientRect();
			const inside =
				event.clientX >= bounds.left &&
				event.clientX <= bounds.right &&
				event.clientY >= bounds.top &&
				event.clientY <= bounds.bottom;

			pointerTarget = inside ? 1 : 0;
			if (!inside) return;

			pointerX = (event.clientX - bounds.left) / bounds.width;
			pointerY = 1 - (event.clientY - bounds.top) / bounds.height;
		};

		const resizeObserver = new ResizeObserver(() => {
			resize();
			if (!animationFrame) draw(performance.now());
		});
		const visibilityObserver = new IntersectionObserver(
			([entry]) => {
				isVisible = entry?.isIntersecting ?? false;
				updateAnimation();
			},
			{ rootMargin: '200px' },
		);

		resizeObserver.observe(canvas);
		visibilityObserver.observe(canvas);
		window.addEventListener('pointermove', handlePointerMove, {
			passive: true,
		});
		document.addEventListener('visibilitychange', updateAnimation);
		reducedMotion.addEventListener('change', updateAnimation);
		updateAnimation();

		return () => {
			stop();
			resizeObserver.disconnect();
			visibilityObserver.disconnect();
			window.removeEventListener('pointermove', handlePointerMove);
			document.removeEventListener('visibilitychange', updateAnimation);
			reducedMotion.removeEventListener('change', updateAnimation);
			context.deleteVertexArray(vertexArray);
			context.deleteProgram(program);
		};
	};
</script>

<div class="shader-background" aria-hidden="true">
	<canvas {@attach mountShader}></canvas>
</div>

<style>
	.shader-background {
		position: absolute;
		z-index: 0;
		inset: 0;
		overflow: hidden;
		pointer-events: none;
	}

	.shader-background::after {
		position: absolute;
		content: '';
		inset: 0;
		background: linear-gradient(
			180deg,
			rgba(8, 8, 10, 0.34) 0%,
			transparent 28%,
			rgba(20, 15, 27, 0.08) 70%,
			rgba(20, 15, 27, 0.2) 100%
		);
	}

	canvas {
		position: absolute;
		width: 104%;
		height: 104%;
		inset: -2%;
		filter: blur(16px) saturate(1.08);
		transform: translateZ(0);
	}

	@media (prefers-reduced-motion: reduce) {
		canvas {
			filter: blur(18px) saturate(1.04);
		}
	}
</style>
