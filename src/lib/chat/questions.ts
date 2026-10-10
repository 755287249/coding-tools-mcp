import type {ChatMessage} from '../api/chat';
/** Plain Markdown fallback for downloaded conversation archives. */
export function questionsMarkdown(message:ChatMessage):string {
  return (message.questions??[]).map(q=>`\n**${q.prompt}**\n${q.options.map(o=>`- ${o.label}${o.description?' — '+o.description:''}\n`).join('')}- 自定义回答\n`).join('');
}
