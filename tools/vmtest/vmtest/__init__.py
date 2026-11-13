"""vmtest - nixosTest-style multi-VM tests for Horcrux and Patronus (stdlib only, QEMU + openssh).

    from vmtest import Test, NodeSpec, Nic, Network, Disk
"""
from .spec import Disk, Network, Nic, NodeSpec, Test      # noqa: F401
from .node import CommandFailed, Node, Timeout, UnitFailed, VmtestError   # noqa: F401
from .machines import Machines, StepFailed             # noqa: F401
from . import cache                                    # noqa: F401

__version__ = "0.1.0"
