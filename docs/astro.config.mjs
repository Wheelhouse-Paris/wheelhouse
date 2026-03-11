// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://docs.wheelhouse.paris',
	integrations: [
		starlight({
			title: 'Wheelhouse',
			description: 'The operating infrastructure for autonomous agent factories',
			logo: {
				light: './src/assets/logo-light.svg',
				dark: './src/assets/logo-dark.svg',
				replacesTitle: false,
			},
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/Wheelhouse-Paris/wheelhouse' },
				{ icon: 'x.com', label: 'X', href: 'https://x.com/wheelhouse_paris' },
			],
			editLink: {
				baseUrl: 'https://github.com/Wheelhouse-Paris/wheelhouse/edit/main/docs/',
			},
			customCss: ['./src/styles/custom.css'],
			sidebar: [
				{
					label: 'Getting Started',
					items: [
						{ label: 'Introduction', slug: 'getting-started/introduction' },
						{ label: 'Installation', slug: 'getting-started/installation' },
						{ label: 'Quick Start', slug: 'getting-started/quick-start' },
					],
				},
				{
					label: 'Concepts',
					items: [
						{ label: 'Overview', slug: 'concepts/overview' },
						{ label: 'Streams', slug: 'concepts/streams' },
						{ label: 'Agents', slug: 'concepts/agents' },
						{ label: 'Surfaces', slug: 'concepts/surfaces' },
						{ label: 'Skills', slug: 'concepts/skills' },
						{ label: 'Cron', slug: 'concepts/cron' },
						{ label: 'Git Backend', slug: 'concepts/git-backend' },
					],
				},
				{
					label: 'Configuration',
					items: [
						{ label: '.wh Files', slug: 'configuration/wh-files' },
						{ label: 'Providers', slug: 'configuration/providers' },
						{ label: 'Guardrails', slug: 'configuration/guardrails' },
					],
				},
				{
					label: 'CLI Reference',
					autogenerate: { directory: 'reference/cli' },
				},
				{
					label: 'SDK',
					items: [
						{ label: 'Python SDK', slug: 'sdk/python' },
					],
				},
				{
					label: 'Guides',
					items: [
						{ label: 'Deploy your first agent', slug: 'guides/first-agent' },
						{ label: 'Build a custom surface', slug: 'guides/custom-surface' },
						{ label: 'Write a skill', slug: 'guides/write-skill' },
					],
				},
			],
		}),
	],
});
