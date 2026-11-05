import { Button } from "../../components/ui";

export function PrivacyNotice({
	onAcknowledge,
	pending,
}: {
	onAcknowledge: () => void;
	pending?: boolean;
}) {
	return (
		<div className="flex flex-col gap-4">
			<ul className="flex flex-col gap-2.5 text-sm leading-relaxed text-muted-foreground">
				<li>
					<strong className="font-medium text-foreground">
						Stored on your Mac.
					</strong>{" "}
					Your projects, chats and settings stay on this computer. Chingui has
					no account and no server.
				</li>
				<li>
					<strong className="font-medium text-foreground">
						Sent to your AI provider.
					</strong>{" "}
					When you send a message, that message and the earlier messages in the
					chat go to the service of the model you chose, Google's Gemini or
					OpenAI, using your own API key for it. That company's terms apply to
					what you send.
				</li>
				<li>
					<strong className="font-medium text-foreground">
						Your key stays private.
					</strong>{" "}
					It is kept in the macOS Keychain, not in a file.
				</li>
				<li>
					<strong className="font-medium text-foreground">One download.</strong>{" "}
					Meaning search uses a 200 MB model that Chingui downloads once from
					Hugging Face, after you agree. Your documents are not sent for it.
				</li>
				<li>
					<strong className="font-medium text-foreground">Price list.</strong>{" "}
					About once a week Chingui downloads a public price list from
					models.dev to estimate costs. The request carries nothing of yours.
				</li>
				<li>
					<strong className="font-medium text-foreground">No tracking.</strong>{" "}
					Chingui doesn't send usage data or your text anywhere else.
				</li>
			</ul>
			<div className="flex justify-end">
				<Button variant="primary" onPress={onAcknowledge} isDisabled={pending}>
					I understand
				</Button>
			</div>
		</div>
	);
}
