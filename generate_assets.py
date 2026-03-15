import shutil
import subprocess
import os
bbtpl_path = shutil.which("bbtpl")
asset_dir = os.path.join(os.getcwd(), "assets_generated")
print("bbtpl: " + bbtpl_path)
print("assets: " + asset_dir)
shutil.rmtree(asset_dir)
os.mkdir(asset_dir)

def generate(name, params):
    arguments = [bbtpl_path, name]
    for key, value in params.items():
        arguments.append(str(key) + "=" + str(value))
    subprocess.Popen(arguments, cwd=asset_dir)
def generate_tag(name, content):
    with open(os.path.join(asset_dir, *name.split(".")) + ".txt", "w") as f:
        f.write("\n".join(content))

wood_types = [{"woodType": "oak"}]
for data in wood_types:
    generate("bb:wood_type", data)
generate_tag("#sticks", ["wood." + data["woodType"] + ".stick" for data in wood_types])

rock_types = [{"rockType": "limestone"}]
for data in rock_types:
    generate("bb:rock_type", data)

ore_types = [{"oreType": "magnetite", "oreColorR": 100, "oreColorG": 100, "oreColorB": 100}]
for ore_data in ore_types:
    generate("bb:ore_type", ore_data)
    for rock_data in rock_types:
        data = dict(ore_data)
        data.update(rock_data)
        generate("bb:rock_ore", data)