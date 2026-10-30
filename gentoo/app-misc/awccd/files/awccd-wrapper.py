#!/usr/bin/python3
import sys
sys.path.insert(0, "/usr/lib/awccd")
if "--restore-state" in sys.argv:
    from awcc import daemon, elc
    daemon.Core(elc=elc.ELC()).restore_on_exit()
    sys.exit(0)
from awcc.daemon import main
sys.exit(main())
