# CLI distribution

The Publish CLI binaries workflow uploads versioned binaries and checksums to S3. Its final upload replaces `install.sh`, which is served through `https://magicmtn.ai/install.sh`. The installer names a complete release and verifies the platform's checksum before installing.

An S3 Object Created event for `install.sh` starts the `magicmtn-cli-homebrew` CodeBuild project. The build reads the release named in the installer, checks that the manifest and installer agree on the Apple Silicon checksum, and updates `Formula/magic.rb` in [UseMagicMtn/homebrew-tap](https://github.com/UseMagicMtn/homebrew-tap) through GitHub's signing API. It checks the resulting commit signature. The formula fetches the same immutable binary as the shell installer. Repeated events for the current release leave the formula untouched.

## Deploy the Homebrew sync

Create a private GitHub App in the UseMagicMtn organization with webhooks disabled and **Contents** set to **Read and write**. Install the app on `UseMagicMtn/homebrew-tap` only. Generate a private key and obtain the app's client ID and installation ID. In AWS Systems Manager Parameter Store in `us-east-2`, create a `SecureString` named `/magicmtn/cli/homebrew-github-app` with a JSON object containing `client_id`, `installation_id`, and `private_key`. The `private_key` value is the full PEM string.

Deploy the stack from the repository root.

```sh
aws cloudformation deploy --region us-east-2 --stack-name magicmtn-cli-homebrew \
  --template-file release/cli-homebrew.yml --capabilities CAPABILITY_IAM
```

Enable EventBridge delivery on the release bucket by adding `"EventBridgeConfiguration": {}` to its existing notification configuration. Preserve the other notification destinations when applying the configuration.

Start the project to sync the current release after setup or to replay a missed event. The build logs are in the `/aws/codebuild/magicmtn-cli-homebrew` CloudWatch log group.

```sh
aws codebuild start-build --region us-east-2 --project-name magicmtn-cli-homebrew
```
