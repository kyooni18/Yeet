from __future__ import annotations

import os
import shlex
import tempfile
from pathlib import Path
from typing import Any, Literal, override

from pydantic import Field

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.agents.model_connection import ModelConnectionSpec
from harbor.agents.options import InstalledAgentOptions
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.trial.paths import EnvironmentPaths


class YeetOptions(InstalledAgentOptions):
    reasoning: Literal["auto", "low", "medium", "high", "xhigh", "max"] = Field(
        default="high",
        description="Yeet reasoning level used for every benchmark task.",
    )
    timeout_seconds: int = Field(
        default=0,
        ge=0,
        description=(
            "Optional Yeet-internal timeout. Zero disables it and leaves timeout "
            "enforcement to Harbor."
        ),
    )


class YeetAgent(BaseInstalledAgent):
    """Run the current local Yeet build as a Harbor/Terminal-Bench agent."""

    options_model = YeetOptions
    MODEL_CONNECTION = ModelConnectionSpec(passthrough=True)

    _REMOTE_ROOT = "/opt/yeet"
    _REMOTE_BUNDLE = "/tmp/yeet-terminalbench-bundle.tar.gz"
    _REMOTE_PROMPT = "/tmp/yeet-terminalbench-instruction.txt"
    _REMOTE_CONFIG = "/tmp/yeet-terminalbench-config"
    _OUTPUT_FILENAME = "yeet.jsonl"

    @staticmethod
    @override
    def name() -> str:
        return "yeet"

    @override
    def get_version_command(self) -> str | None:
        return "yeet --version"

    @staticmethod
    def _normalized_arch(machine: str) -> str:
        machine = machine.strip().lower()
        if machine in {"aarch64", "arm64"}:
            return "arm64"
        if machine in {"x86_64", "amd64"}:
            return "amd64"
        raise RuntimeError(f"Unsupported Terminal-Bench architecture for Yeet: {machine}")

    def _bundle_path(self, arch: str) -> Path:
        override = os.environ.get("YEET_HARBOR_BUNDLE")
        if override:
            path = Path(override).expanduser().resolve()
        else:
            path = Path(__file__).resolve().parent / "dist" / f"linux-{arch}" / "yeet-bundle.tar.gz"
        if not path.is_file():
            raise FileNotFoundError(
                f"Yeet benchmark bundle is missing: {path}. "
                f"Build it with ./benchmarks/terminalbench/build_bundle.sh {arch}"
            )
        return path

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        arch_result = await environment.exec("uname -m")
        if arch_result.return_code != 0:
            raise RuntimeError(f"Unable to detect task architecture: {arch_result.stderr}")
        arch = self._normalized_arch(arch_result.stdout or "")
        bundle = self._bundle_path(arch)

        # Terminal-Bench images already provide the shell/tooling Harbor needs.
        # Installing a generic dependency set here is extremely expensive on
        # Apple Silicon because most TB2 images are amd64 and apt/dpkg then run
        # under QEMU. Yeet itself is self-contained (static Rust binary + Node).
        # Only fall back to package installation if the one extraction tool we
        # actually require is missing.
        tar_probe = await environment.exec("command -v tar >/dev/null 2>&1")
        if tar_probe.return_code != 0:
            await self.ensure_system_dependencies(environment, ("tar",))

        await environment.upload_file(bundle, self._REMOTE_BUNDLE)
        await self.exec_as_root(
            environment,
            command=(
                f"rm -rf {shlex.quote(self._REMOTE_ROOT)} && "
                f"mkdir -p {shlex.quote(self._REMOTE_ROOT)} && "
                f"tar -xzf {shlex.quote(self._REMOTE_BUNDLE)} "
                f"-C {shlex.quote(self._REMOTE_ROOT)} && "
                f"chmod 0755 {shlex.quote(self._REMOTE_ROOT)}/yeet "
                f"{shlex.quote(self._REMOTE_ROOT)}/node && "
                f"ln -sf {shlex.quote(self._REMOTE_ROOT)}/yeet /usr/local/bin/yeet"
            ),
        )
        await self.exec_as_agent(
            environment,
            command=(
                f"{shlex.quote(self._REMOTE_ROOT)}/node --version && "
                f"YEET_RUNTIME_DIR={shlex.quote(self._REMOTE_ROOT)}/runtime "
                f"YEET_NODE={shlex.quote(self._REMOTE_ROOT)}/node "
                "yeet --version"
            ),
        )

    async def _upload_instruction(self, environment: BaseEnvironment, instruction: str) -> None:
        with tempfile.TemporaryDirectory(prefix="yeet-terminalbench-") as temp_dir:
            local_path = Path(temp_dir) / "instruction.txt"
            local_path.write_text(instruction, encoding="utf-8")
            await environment.upload_file(local_path, self._REMOTE_PROMPT)

        if environment.default_user is not None:
            owner = shlex.quote(str(environment.default_user))
            await self.exec_as_root(
                environment,
                command=(
                    f"chown {owner} {shlex.quote(self._REMOTE_PROMPT)} && "
                    f"chmod 0600 {shlex.quote(self._REMOTE_PROMPT)}"
                ),
            )

    @override
    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        del context
        if not self.model_name:
            raise ValueError("Yeet Terminal-Bench runs require --model provider/model")
        if not isinstance(self.options, YeetOptions):
            raise RuntimeError("Yeet options were not initialized by Harbor")

        await self._upload_instruction(environment, instruction)
        output_path = (EnvironmentPaths.agent_dir / self._OUTPUT_FILENAME).as_posix()
        env = {
            "YEET_RUNTIME_DIR": f"{self._REMOTE_ROOT}/runtime",
            "YEET_NODE": f"{self._REMOTE_ROOT}/node",
            "YEET_CONFIG_DIR": self._REMOTE_CONFIG,
            "YEET_AGENT_TIMEOUT_SECONDS": str(self.options.timeout_seconds),
        }
        access = self.model_connection
        env.update(access.env)
        provider = self.model_name.split("/", 1)[0]
        yeet_key_env = {
            "openai": "OPENAI_API_KEY",
            "anthropic": "ANTHROPIC_API_KEY",
            "gemini": "GEMINI_API_KEY",
            "opencode": "OPENCODE_API_KEY",
            "opencode-go": "OPENCODE_API_KEY",
            "openrouter": "OPENROUTER_API_KEY",
        }.get(provider)
        if access.api_key and yeet_key_env:
            env[yeet_key_env] = access.api_key

        command = (
            f"mkdir -p {shlex.quote(self._REMOTE_CONFIG)} "
            f"{shlex.quote(EnvironmentPaths.agent_dir.as_posix())} && "
            "yeet agent --benchmark --json "
            f"--model {shlex.quote(self.model_name)} "
            f"--reasoning {shlex.quote(self.options.reasoning)} "
            f"--timeout-seconds {self.options.timeout_seconds} "
            f"< {shlex.quote(self._REMOTE_PROMPT)} "
            f"2>&1 | tee {shlex.quote(output_path)}"
        )
        await self.exec_as_agent(environment, command=command, env=env)
